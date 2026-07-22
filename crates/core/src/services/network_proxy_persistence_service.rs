use super::ConfigService;
use anyhow::{anyhow, Context, Result};
use arclain_db::{SecretMutation, SecretsDb, UserConfig};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

const PROXY_PASSWORD_KEY: &str = "proxy:socks5";
const PENDING_PROXY_UPDATE_KEY: &str = "journal:proxy-settings";
const PROXY_UPDATE_MARKER_VERSION: u8 = 1;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProxySettingsSnapshot {
    enabled: bool,
    address: Option<String>,
    username: Option<String>,
}

impl From<&UserConfig> for ProxySettingsSnapshot {
    fn from(config: &UserConfig) -> Self {
        Self {
            enabled: config.socks5_enabled,
            address: config.socks5_address.clone(),
            username: config.socks5_username.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct PendingProxyUpdate {
    version: u8,
    previous: ProxySettingsSnapshot,
    candidate: ProxySettingsSnapshot,
    previous_password: Option<String>,
}

impl Drop for PendingProxyUpdate {
    fn drop(&mut self) {
        self.previous.address.zeroize();
        self.previous.username.zeroize();
        self.candidate.address.zeroize();
        self.candidate.username.zeroize();
        self.previous_password.zeroize();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxySaveOutcome {
    Committed,
    CommittedPendingFinalize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyRecoveryOutcome {
    NoPendingUpdate,
    RolledBack,
    Finalized,
}

trait ProxyConfigStore {
    fn load_user_config(&self) -> Result<UserConfig>;
    fn save_user_config(&self, config: &UserConfig) -> Result<()>;
}

impl ProxyConfigStore for ConfigService {
    fn load_user_config(&self) -> Result<UserConfig> {
        self.get_user_config()
    }

    fn save_user_config(&self, config: &UserConfig) -> Result<()> {
        ConfigService::save_user_config(self, config)
    }
}

trait ProxySecretStore {
    fn read_secret(&self, key: &str) -> Result<Option<Zeroizing<String>>>;
    fn apply_secret_mutations(&self, mutations: &[SecretMutation<'_>]) -> Result<()>;
}

impl ProxySecretStore for SecretsDb {
    fn read_secret(&self, key: &str) -> Result<Option<Zeroizing<String>>> {
        self.get_secret(key)
    }

    fn apply_secret_mutations(&self, mutations: &[SecretMutation<'_>]) -> Result<()> {
        SecretsDb::apply_secret_mutations(self, mutations)
    }
}

pub struct NetworkProxyPersistenceService<'a> {
    config: &'a dyn ProxyConfigStore,
    secrets: &'a dyn ProxySecretStore,
}

impl<'a> NetworkProxyPersistenceService<'a> {
    pub fn new(config: &'a ConfigService, secrets: &'a SecretsDb) -> Self {
        Self { config, secrets }
    }

    #[cfg(test)]
    fn from_stores(config: &'a dyn ProxyConfigStore, secrets: &'a dyn ProxySecretStore) -> Self {
        Self { config, secrets }
    }

    pub fn save(&self, candidate: &UserConfig, password: Option<&str>) -> Result<ProxySaveOutcome> {
        self.recover_pending()
            .context("recovering an earlier proxy settings update")?;

        let previous = self
            .config
            .load_user_config()
            .context("loading previous proxy settings")?;
        let previous_password = self
            .secrets
            .read_secret(PROXY_PASSWORD_KEY)
            .context("loading previous proxy password")?;
        let marker = PendingProxyUpdate {
            version: PROXY_UPDATE_MARKER_VERSION,
            previous: ProxySettingsSnapshot::from(&previous),
            candidate: ProxySettingsSnapshot::from(candidate),
            previous_password: previous_password
                .as_ref()
                .map(|value| value.as_str().to_owned()),
        };
        let marker_json = Zeroizing::new(
            serde_json::to_string(&marker).context("serializing proxy update marker")?,
        );

        let password_mutation = match password {
            Some(value) => SecretMutation::Set {
                key: PROXY_PASSWORD_KEY,
                value,
            },
            None => SecretMutation::Remove {
                key: PROXY_PASSWORD_KEY,
            },
        };
        self.secrets
            .apply_secret_mutations(&[
                password_mutation,
                SecretMutation::Set {
                    key: PENDING_PROXY_UPDATE_KEY,
                    value: marker_json.as_str(),
                },
            ])
            .context("preparing proxy settings update")?;

        if let Err(config_error) = self.config.save_user_config(candidate) {
            let persisted = self
                .config
                .load_user_config()
                .map(|config| ProxySettingsSnapshot::from(&config));
            match persisted {
                Ok(snapshot) if snapshot == marker.candidate => {
                    return Ok(self.finish_commit());
                }
                Ok(snapshot) if snapshot == marker.previous => {
                    if self.rollback(&marker).is_err() {
                        return Err(anyhow!(
                            "proxy settings were not saved; encrypted rollback remains pending recovery"
                        ));
                    }
                    return Err(config_error).context("saving proxy settings");
                }
                _ => {
                    return Err(anyhow!(
                        "proxy settings save outcome is uncertain; recovery remains pending"
                    ));
                }
            }
        }

        Ok(self.finish_commit())
    }

    pub fn recover_pending(&self) -> Result<ProxyRecoveryOutcome> {
        let Some(marker_json) = self
            .secrets
            .read_secret(PENDING_PROXY_UPDATE_KEY)
            .context("reading pending proxy update marker")?
        else {
            return Ok(ProxyRecoveryOutcome::NoPendingUpdate);
        };
        let marker: PendingProxyUpdate = serde_json::from_str(marker_json.as_str())
            .context("parsing pending proxy update marker")?;
        if marker.version != PROXY_UPDATE_MARKER_VERSION {
            return Err(anyhow!("unsupported pending proxy update marker version"));
        }

        let persisted = self
            .config
            .load_user_config()
            .context("loading proxy settings during recovery")?;
        let persisted = ProxySettingsSnapshot::from(&persisted);

        if persisted == marker.candidate {
            self.finalize()
                .context("finalizing pending proxy settings update")?;
            return Ok(ProxyRecoveryOutcome::Finalized);
        }
        if persisted == marker.previous {
            self.rollback(&marker)
                .context("rolling back pending proxy settings update")?;
            return Ok(ProxyRecoveryOutcome::RolledBack);
        }

        Err(anyhow!(
            "pending proxy update does not match persisted proxy settings"
        ))
    }

    fn rollback(&self, marker: &PendingProxyUpdate) -> Result<()> {
        let password_mutation = match marker.previous_password.as_deref() {
            Some(value) => SecretMutation::Set {
                key: PROXY_PASSWORD_KEY,
                value,
            },
            None => SecretMutation::Remove {
                key: PROXY_PASSWORD_KEY,
            },
        };
        self.secrets.apply_secret_mutations(&[
            password_mutation,
            SecretMutation::Remove {
                key: PENDING_PROXY_UPDATE_KEY,
            },
        ])
    }

    fn finalize(&self) -> Result<()> {
        self.secrets
            .apply_secret_mutations(&[SecretMutation::Remove {
                key: PENDING_PROXY_UPDATE_KEY,
            }])
    }

    fn finish_commit(&self) -> ProxySaveOutcome {
        if self.finalize().is_err() {
            ProxySaveOutcome::CommittedPendingFinalize
        } else {
            ProxySaveOutcome::Committed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{anyhow, Result};
    use arclain_db::{open_databases, DbConnection, DbPaths, SecretMutation, SecretsKey};
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zeroize::Zeroizing;

    #[derive(Clone, Copy)]
    enum SaveBehavior {
        Succeed,
        Error,
        CommitThenError,
        Panic,
    }

    struct FakeConfigStore {
        current: Mutex<UserConfig>,
        behavior: Mutex<SaveBehavior>,
    }

    impl FakeConfigStore {
        fn new(current: UserConfig) -> Self {
            Self {
                current: Mutex::new(current),
                behavior: Mutex::new(SaveBehavior::Succeed),
            }
        }

        fn set_behavior(&self, behavior: SaveBehavior) {
            *self.behavior.lock() = behavior;
        }

        fn persisted(&self) -> UserConfig {
            self.current.lock().clone()
        }
    }

    impl ProxyConfigStore for FakeConfigStore {
        fn load_user_config(&self) -> Result<UserConfig> {
            Ok(self.persisted())
        }

        fn save_user_config(&self, config: &UserConfig) -> Result<()> {
            match *self.behavior.lock() {
                SaveBehavior::Succeed => {
                    *self.current.lock() = config.clone();
                    Ok(())
                }
                SaveBehavior::Error => Err(anyhow!("injected config save failure")),
                SaveBehavior::CommitThenError => {
                    *self.current.lock() = config.clone();
                    Err(anyhow!("injected post-commit config error"))
                }
                SaveBehavior::Panic => panic!("simulated process crash during config save"),
            }
        }
    }

    struct FakeSecretStore {
        values: Mutex<HashMap<String, String>>,
        apply_calls: AtomicUsize,
        fail_apply_call: AtomicUsize,
        panic_apply_call: AtomicUsize,
    }

    impl FakeSecretStore {
        fn new(proxy_password: Option<&str>) -> Self {
            let mut values = HashMap::new();
            if let Some(password) = proxy_password {
                values.insert(PROXY_PASSWORD_KEY.to_string(), password.to_string());
            }
            Self {
                values: Mutex::new(values),
                apply_calls: AtomicUsize::new(0),
                fail_apply_call: AtomicUsize::new(0),
                panic_apply_call: AtomicUsize::new(0),
            }
        }

        fn fail_on_apply(&self, call: usize) {
            self.fail_apply_call.store(call, Ordering::SeqCst);
        }

        fn panic_on_apply(&self, call: usize) {
            self.panic_apply_call.store(call, Ordering::SeqCst);
        }

        fn clear_faults(&self) {
            self.fail_apply_call.store(0, Ordering::SeqCst);
            self.panic_apply_call.store(0, Ordering::SeqCst);
        }

        fn value(&self, key: &str) -> Option<String> {
            self.values.lock().get(key).cloned()
        }
    }

    impl ProxySecretStore for FakeSecretStore {
        fn read_secret(&self, key: &str) -> Result<Option<Zeroizing<String>>> {
            Ok(self.value(key).map(Zeroizing::new))
        }

        fn apply_secret_mutations(&self, mutations: &[SecretMutation<'_>]) -> Result<()> {
            let call = self.apply_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.panic_apply_call.load(Ordering::SeqCst) == call {
                panic!("simulated process crash during secret mutation");
            }
            if self.fail_apply_call.load(Ordering::SeqCst) == call {
                return Err(anyhow!("injected secret mutation failure"));
            }

            let mut candidate = self.values.lock().clone();
            for mutation in mutations {
                match mutation {
                    SecretMutation::Set { key, value } => {
                        candidate.insert((*key).to_string(), (*value).to_string());
                    }
                    SecretMutation::Remove { key } => {
                        candidate.remove(*key);
                    }
                }
            }
            *self.values.lock() = candidate;
            Ok(())
        }
    }

    fn proxy_config(address: &str, username: &str) -> UserConfig {
        let mut config = UserConfig::new();
        config.socks5_enabled = true;
        config.socks5_address = Some(address.to_string());
        config.socks5_username = Some(username.to_string());
        config
    }

    fn assert_proxy_config(actual: &UserConfig, expected: &UserConfig) {
        assert_eq!(actual.socks5_enabled, expected.socks5_enabled);
        assert_eq!(actual.socks5_address, expected.socks5_address);
        assert_eq!(actual.socks5_username, expected.socks5_username);
    }

    fn open_real_stores(
        paths: &DbPaths,
        key: &SecretsKey,
    ) -> (arclain_db::ConfigDbs, ConfigService) {
        let dbs = open_databases(paths, key).expect("open test databases");
        let connection = DbConnection::open(&paths.config_db).expect("open config connection");
        UserConfig::ensure_table(&connection).expect("ensure user config table");
        let config = ConfigService::from_connection(dbs.config_pool.clone(), connection);
        (dbs, config)
    }

    fn prepare_real_update(
        secrets: &SecretsDb,
        previous: &UserConfig,
        candidate: &UserConfig,
        previous_password: &str,
        candidate_password: &str,
    ) {
        let marker = PendingProxyUpdate {
            version: PROXY_UPDATE_MARKER_VERSION,
            previous: ProxySettingsSnapshot::from(previous),
            candidate: ProxySettingsSnapshot::from(candidate),
            previous_password: Some(previous_password.to_string()),
        };
        let marker_json = Zeroizing::new(serde_json::to_string(&marker).unwrap());
        secrets
            .apply_secret_mutations(&[
                SecretMutation::Set {
                    key: PROXY_PASSWORD_KEY,
                    value: candidate_password,
                },
                SecretMutation::Set {
                    key: PENDING_PROXY_UPDATE_KEY,
                    value: marker_json.as_str(),
                },
            ])
            .unwrap();
    }

    #[test]
    fn recovery_without_marker_is_an_idempotent_noop() {
        let config = FakeConfigStore::new(proxy_config("old:1080", "old-user"));
        let secrets = FakeSecretStore::new(Some("old-password"));
        let service = NetworkProxyPersistenceService::from_stores(&config, &secrets);

        assert_eq!(
            service.recover_pending().unwrap(),
            ProxyRecoveryOutcome::NoPendingUpdate
        );
        assert_eq!(
            service.recover_pending().unwrap(),
            ProxyRecoveryOutcome::NoPendingUpdate
        );
        assert_eq!(
            secrets.value(PROXY_PASSWORD_KEY).as_deref(),
            Some("old-password")
        );
    }

    #[test]
    fn crash_after_prepare_before_config_rolls_back_on_recovery() {
        let previous = proxy_config("old:1080", "old-user");
        let candidate = proxy_config("new:1080", "new-user");
        let config = FakeConfigStore::new(previous.clone());
        config.set_behavior(SaveBehavior::Panic);
        let secrets = FakeSecretStore::new(Some("old-password"));
        let service = NetworkProxyPersistenceService::from_stores(&config, &secrets);

        assert!(catch_unwind(AssertUnwindSafe(|| {
            let _ = service.save(&candidate, Some("new-password"));
        }))
        .is_err());
        assert_eq!(
            secrets.value(PROXY_PASSWORD_KEY).as_deref(),
            Some("new-password")
        );
        assert!(secrets.value(PENDING_PROXY_UPDATE_KEY).is_some());

        config.set_behavior(SaveBehavior::Succeed);
        assert_eq!(
            service.recover_pending().unwrap(),
            ProxyRecoveryOutcome::RolledBack
        );
        assert_proxy_config(&config.persisted(), &previous);
        assert_eq!(
            secrets.value(PROXY_PASSWORD_KEY).as_deref(),
            Some("old-password")
        );
        assert!(secrets.value(PENDING_PROXY_UPDATE_KEY).is_none());
        assert_eq!(
            service.recover_pending().unwrap(),
            ProxyRecoveryOutcome::NoPendingUpdate
        );
    }

    #[test]
    fn crash_after_config_before_finalize_finishes_commit_on_recovery() {
        let previous = proxy_config("old:1080", "old-user");
        let candidate = proxy_config("new:1080", "new-user");
        let config = FakeConfigStore::new(previous);
        let secrets = FakeSecretStore::new(Some("old-password"));
        secrets.panic_on_apply(2);
        let service = NetworkProxyPersistenceService::from_stores(&config, &secrets);

        assert!(catch_unwind(AssertUnwindSafe(|| {
            let _ = service.save(&candidate, Some("new-password"));
        }))
        .is_err());
        assert_proxy_config(&config.persisted(), &candidate);
        assert_eq!(
            secrets.value(PROXY_PASSWORD_KEY).as_deref(),
            Some("new-password")
        );
        assert!(secrets.value(PENDING_PROXY_UPDATE_KEY).is_some());

        secrets.clear_faults();
        assert_eq!(
            service.recover_pending().unwrap(),
            ProxyRecoveryOutcome::Finalized
        );
        assert!(secrets.value(PENDING_PROXY_UPDATE_KEY).is_none());
        assert_eq!(
            service.recover_pending().unwrap(),
            ProxyRecoveryOutcome::NoPendingUpdate
        );
    }

    #[test]
    fn config_failure_rolls_back_secret_and_marker() {
        let previous = proxy_config("old:1080", "old-user");
        let candidate = proxy_config("new:1080", "new-user");
        let config = FakeConfigStore::new(previous.clone());
        config.set_behavior(SaveBehavior::Error);
        let secrets = FakeSecretStore::new(Some("old-password"));
        let service = NetworkProxyPersistenceService::from_stores(&config, &secrets);

        assert!(service.save(&candidate, Some("new-password")).is_err());

        assert_proxy_config(&config.persisted(), &previous);
        assert_eq!(
            secrets.value(PROXY_PASSWORD_KEY).as_deref(),
            Some("old-password")
        );
        assert!(secrets.value(PENDING_PROXY_UPDATE_KEY).is_none());
    }

    #[test]
    fn post_commit_config_error_is_reconciled_before_rollback() {
        let previous = proxy_config("old:1080", "old-user");
        let candidate = proxy_config("new:1080", "new-user");
        let config = FakeConfigStore::new(previous);
        config.set_behavior(SaveBehavior::CommitThenError);
        let secrets = FakeSecretStore::new(Some("old-password"));
        let service = NetworkProxyPersistenceService::from_stores(&config, &secrets);

        assert_eq!(
            service.save(&candidate, Some("new-password")).unwrap(),
            ProxySaveOutcome::Committed
        );
        assert_proxy_config(&config.persisted(), &candidate);
        assert_eq!(
            secrets.value(PROXY_PASSWORD_KEY).as_deref(),
            Some("new-password")
        );
        assert!(secrets.value(PENDING_PROXY_UPDATE_KEY).is_none());
    }

    #[test]
    fn rollback_failure_retains_prepared_marker_for_recovery() {
        let previous = proxy_config("old:1080", "old-user");
        let candidate = proxy_config("new:1080", "new-user");
        let config = FakeConfigStore::new(previous.clone());
        config.set_behavior(SaveBehavior::Error);
        let secrets = FakeSecretStore::new(Some("old-password"));
        secrets.fail_on_apply(2);
        let service = NetworkProxyPersistenceService::from_stores(&config, &secrets);

        let error = service.save(&candidate, Some("new-password")).unwrap_err();

        assert!(error.to_string().contains("rollback remains pending"));
        assert_proxy_config(&config.persisted(), &previous);
        assert_eq!(
            secrets.value(PROXY_PASSWORD_KEY).as_deref(),
            Some("new-password")
        );
        assert!(secrets.value(PENDING_PROXY_UPDATE_KEY).is_some());

        secrets.clear_faults();
        assert_eq!(
            service.recover_pending().unwrap(),
            ProxyRecoveryOutcome::RolledBack
        );
        assert_eq!(
            secrets.value(PROXY_PASSWORD_KEY).as_deref(),
            Some("old-password")
        );
        assert!(secrets.value(PENDING_PROXY_UPDATE_KEY).is_none());
    }

    #[test]
    fn finalize_failure_is_reported_as_committed_pending_finalize() {
        let previous = proxy_config("old:1080", "old-user");
        let candidate = proxy_config("new:1080", "new-user");
        let config = FakeConfigStore::new(previous);
        let secrets = FakeSecretStore::new(Some("old-password"));
        secrets.fail_on_apply(2);
        let service = NetworkProxyPersistenceService::from_stores(&config, &secrets);

        assert_eq!(
            service.save(&candidate, Some("new-password")).unwrap(),
            ProxySaveOutcome::CommittedPendingFinalize
        );
        assert_proxy_config(&config.persisted(), &candidate);
        assert_eq!(
            secrets.value(PROXY_PASSWORD_KEY).as_deref(),
            Some("new-password")
        );
        assert!(secrets.value(PENDING_PROXY_UPDATE_KEY).is_some());

        secrets.clear_faults();
        assert_eq!(
            service.recover_pending().unwrap(),
            ProxyRecoveryOutcome::Finalized
        );
    }

    #[test]
    fn ambiguous_config_retains_marker_and_fails_closed() {
        let previous = proxy_config("old:1080", "old-user");
        let candidate = proxy_config("new:1080", "new-user");
        let config = FakeConfigStore::new(previous);
        config.set_behavior(SaveBehavior::Panic);
        let secrets = FakeSecretStore::new(Some("old-password"));
        let service = NetworkProxyPersistenceService::from_stores(&config, &secrets);
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _ = service.save(&candidate, Some("new-password"));
        }));
        *config.current.lock() = proxy_config("third:1080", "third-user");

        let error = service.recover_pending().unwrap_err();

        assert!(error.to_string().contains("does not match"));
        assert!(secrets.value(PENDING_PROXY_UPDATE_KEY).is_some());
        assert_eq!(
            secrets.value(PROXY_PASSWORD_KEY).as_deref(),
            Some("new-password")
        );
    }

    #[test]
    fn reopened_real_stores_roll_back_a_prepared_update() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DbPaths {
            config_db: temp.path().join("config.sqlite"),
            cache_db: temp.path().join("metadata.sqlite"),
            secrets_db: temp.path().join("secrets.redb"),
            key_file: None,
        };
        let key = SecretsKey::generate();
        let previous = proxy_config("old:1080", "old-user");
        let candidate = proxy_config("new:1080", "new-user");
        {
            let (dbs, config) = open_real_stores(&paths, &key);
            config.save_user_config(&previous).unwrap();
            dbs.secrets
                .set_secret(PROXY_PASSWORD_KEY, "old-password")
                .unwrap();
            prepare_real_update(
                &dbs.secrets,
                &previous,
                &candidate,
                "old-password",
                "new-password",
            );
        }

        let (dbs, config) = open_real_stores(&paths, &key);
        let service = NetworkProxyPersistenceService::new(&config, &dbs.secrets);
        assert_eq!(
            service.recover_pending().unwrap(),
            ProxyRecoveryOutcome::RolledBack
        );
        assert_proxy_config(&config.get_user_config().unwrap(), &previous);
        assert_eq!(
            dbs.secrets
                .get_secret(PROXY_PASSWORD_KEY)
                .unwrap()
                .as_ref()
                .map(|value| value.as_str()),
            Some("old-password")
        );
        assert!(dbs
            .secrets
            .get_secret(PENDING_PROXY_UPDATE_KEY)
            .unwrap()
            .is_none());
    }

    #[test]
    fn reopened_real_stores_finalize_a_committed_update() {
        let temp = tempfile::tempdir().unwrap();
        let paths = DbPaths {
            config_db: temp.path().join("config.sqlite"),
            cache_db: temp.path().join("metadata.sqlite"),
            secrets_db: temp.path().join("secrets.redb"),
            key_file: None,
        };
        let key = SecretsKey::generate();
        let previous = proxy_config("old:1080", "old-user");
        let candidate = proxy_config("new:1080", "new-user");
        {
            let (dbs, config) = open_real_stores(&paths, &key);
            config.save_user_config(&previous).unwrap();
            dbs.secrets
                .set_secret(PROXY_PASSWORD_KEY, "old-password")
                .unwrap();
            prepare_real_update(
                &dbs.secrets,
                &previous,
                &candidate,
                "old-password",
                "new-password",
            );
            config.save_user_config(&candidate).unwrap();
        }

        let (dbs, config) = open_real_stores(&paths, &key);
        let service = NetworkProxyPersistenceService::new(&config, &dbs.secrets);
        assert_eq!(
            service.recover_pending().unwrap(),
            ProxyRecoveryOutcome::Finalized
        );
        assert_proxy_config(&config.get_user_config().unwrap(), &candidate);
        assert_eq!(
            dbs.secrets
                .get_secret(PROXY_PASSWORD_KEY)
                .unwrap()
                .as_ref()
                .map(|value| value.as_str()),
            Some("new-password")
        );
        assert!(dbs
            .secrets
            .get_secret(PENDING_PROXY_UPDATE_KEY)
            .unwrap()
            .is_none());
    }
}
