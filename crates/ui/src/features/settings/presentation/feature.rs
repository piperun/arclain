use crate::core::SettingsPage;
use crate::features::password_management::dialogs::PasswordRulesDialog;
use crate::features::settings::pages::interface::InterfaceSettingsState;
use crate::features::settings::pages::keyboard_mouse::KeyboardMouseSettingsState;
use crate::features::settings::pages::{InfoPanelLayoutState, ToolbarLayoutState};
use crate::features::settings::presentation::views::settings_content::{
    render_settings_content, ArchivesSettingsState, GeneralSettingsState, NetworkSettingsState,
    SecuritySettingsState, ServerSettingsState, SettingsAction,
};

use crate::features::settings::views::{header, layout, navigation};
use crate::shared::SharedState;
use arclain_signals::Signal;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct SettingsFeature {
    pub general_state: GeneralSettingsState,
    pub network_state: NetworkSettingsState,
    pub server_state: ServerSettingsState,
    pub security_state: SecuritySettingsState,
    pub archives_state: ArchivesSettingsState,
    pub password_rules_dialog: PasswordRulesDialog,
    pub plugins_state: crate::features::plugins::domain::types::PluginsListState,

    pub interface_state: InterfaceSettingsState,
    pub toolbar_layout_state: ToolbarLayoutState,
    pub info_panel_layout_state: InfoPanelLayoutState,
    pub keyboard_mouse_state: KeyboardMouseSettingsState,
    pub last_visited_page: Option<SettingsPage>,

    /// Cached dirty state for rule editor (synced from RulesPage each frame)
    pub rule_editor_dirty: bool,
    /// Whether connection_test_status signal has been bound to egui context
    signals_bound: AtomicBool,
}

impl SettingsFeature {
    pub fn new(shared: &SharedState) -> Self {
        // Load saved settings from config signal
        let user_config = shared.signals().user_config.get();
        let open_nested_in_new_tab = user_config.open_nested_in_new_tab;
        let drop_behavior = arclain_core::DropBehavior::from_str(
            user_config.drop_behavior.as_deref().unwrap_or("new_tab"),
        );

        let rules = {
            let state = shared.app_state.lock();
            state
                .pass_rules
                .iter()
                .map(|r| {
                    crate::features::password_management::dialogs::zip_pass_rules::PasswordRule {
                        name: r.name.clone(),
                        pattern: r.pattern.clone(),
                        password: r.password.clone(),
                        priority: r.priority,
                        enabled: r.enabled,
                    }
                })
                .collect()
        };

        let network_state = {
            let state = shared.app_state.lock();
            let password = if let Some(dbs) = &state.dbs {
                dbs.secrets
                    .get_secret("proxy:socks5")
                    .unwrap_or(None)
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            drop(state);

            use crate::features::settings::domain::types::ConnectionTestStatus;

            use arclain_signals::Signal;

            NetworkSettingsState {
                socks5_enabled: Signal::new(user_config.socks5_enabled),
                socks5_address: Signal::new(user_config.socks5_address.clone().unwrap_or_default()),
                socks5_username: Signal::new(
                    user_config.socks5_username.clone().unwrap_or_default(),
                ),
                socks5_password: Signal::new(password),
                connection_test_status: Signal::new(ConnectionTestStatus::Idle),
            }
        };

        let server_state = {
            use crate::features::settings::domain::types::ServerConnectionStatus;
            use arclain_signals::Signal;

            let state = shared.app_state.lock();
            let api_key = if let Some(dbs) = &state.dbs {
                dbs.secrets
                    .get_secret("gameta:api_key")
                    .unwrap_or(None)
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            drop(state);

            ServerSettingsState {
                enabled: Signal::new(user_config.gameta_server_enabled),
                url: Signal::new(
                    user_config.gameta_server_url.clone().unwrap_or_default(),
                ),
                api_key: Signal::new(api_key),
                connection_status: Signal::new(ServerConnectionStatus::Idle),
            }
        };

        Self {
            general_state: GeneralSettingsState {
                open_nested_in_new_tab: Signal::new(open_nested_in_new_tab),
                drop_behavior: Signal::new(drop_behavior),
            },
            network_state,
            server_state,
            security_state: SecuritySettingsState::default(),
            archives_state: ArchivesSettingsState::default(),
            password_rules_dialog: PasswordRulesDialog {
                rules,
                ..Default::default()
            },
            plugins_state: crate::features::plugins::domain::types::PluginsListState::default(),

            interface_state: InterfaceSettingsState::default(),
            toolbar_layout_state: ToolbarLayoutState::default(),
            info_panel_layout_state: InfoPanelLayoutState::default(),
            keyboard_mouse_state: KeyboardMouseSettingsState::new(),
            last_visited_page: None,
            rule_editor_dirty: false,
            signals_bound: AtomicBool::new(false),
        }
    }

    /// Bind signals to egui context for automatic repaints (called once)
    fn bind_signals(&self, ctx: &egui::Context) {
        if self.signals_bound.swap(true, Ordering::SeqCst) {
            return; // Already bound
        }
        let ctx_network = ctx.clone();
        self.network_state.connection_test_status.subscribe(move || {
            ctx_network.request_repaint();
        });
        let ctx_server = ctx.clone();
        self.server_state.connection_status.subscribe(move || {
            ctx_server.request_repaint();
        });
    }

    pub fn check_changes(&self, shared: &SharedState, page: &SettingsPage) -> bool {
        let state = shared.app_state.lock();

        match page {
            SettingsPage::General => {
                let stored_drop = arclain_core::DropBehavior::from_str(
                    state.user_config.drop_behavior.as_deref().unwrap_or("new_tab"),
                );
                *self.general_state.open_nested_in_new_tab.read()
                    != state.user_config.open_nested_in_new_tab
                    || *self.general_state.drop_behavior.read() != stored_drop
            }
            SettingsPage::Archives => {
                let current_val = if self.archives_state.temp_dir.read().trim().is_empty() {
                    None
                } else {
                    Some(self.archives_state.temp_dir.read().trim().to_string())
                };
                current_val != state.user_config.temp_dir
            }
            SettingsPage::Network => {
                *self.network_state.socks5_enabled.read() != state.user_config.socks5_enabled
                    || *self.network_state.socks5_address.read()
                        != state.user_config.socks5_address.clone().unwrap_or_default()
                    || *self.network_state.socks5_username.read()
                        != state
                            .user_config
                            .socks5_username
                            .clone()
                            .unwrap_or_default()
            }
            SettingsPage::Server => {
                *self.server_state.enabled.read() != state.user_config.gameta_server_enabled
                    || *self.server_state.url.read()
                        != state
                            .user_config
                            .gameta_server_url
                            .clone()
                            .unwrap_or_default()
            }
            SettingsPage::Security => {
                !self.security_state.key_file_path.read().trim().is_empty()
                    || !self.security_state.secrets_db_path.read().trim().is_empty()
                    || *self.security_state.encrypted_crc_policy.read()
                        != crate::features::settings::domain::types::EncryptedCrcPolicy::default()
            }
            SettingsPage::PasswordRules => {
                if self.password_rules_dialog.rules.len() != state.pass_rules.len() {
                    return true;
                }
                for (i, rule) in self.password_rules_dialog.rules.iter().enumerate() {
                    let other = &state.pass_rules[i];
                    if rule.name != other.name
                        || rule.pattern != other.pattern
                        || rule.password != other.password
                        || rule.priority != other.priority
                        || rule.enabled != other.enabled
                    {
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        shared: &SharedState,
        page: &SettingsPage,
        breadcrumb: Vec<(String, crate::core::AppPage)>,
        rules_page: Option<&mut crate::features::settings::presentation::pages::RulesPage>,
        profiles_page: Option<&mut crate::features::settings::presentation::pages::ProfilesPage>,

        search_text: &str,
    ) -> Option<crate::core::AppPage> {
        // Bind signals to egui context (once)
        self.bind_signals(ui.ctx());

        // Sync rule editor dirty state for header
        self.rule_editor_dirty = rules_page.as_ref()
            .map(|rp| rp.is_editor_dirty())
            .unwrap_or(false);

        // Sync rules if entering PasswordRules page
        if *page == SettingsPage::PasswordRules && self.last_visited_page.as_ref() != Some(page) {
            let state = shared.app_state.lock();
            self.password_rules_dialog.rules = state
                .pass_rules
                .iter()
                .map(|r| {
                    crate::features::password_management::dialogs::zip_pass_rules::PasswordRule {
                        name: r.name.clone(),
                        pattern: r.pattern.clone(),
                        password: r.password.clone(),
                        priority: r.priority,
                        enabled: r.enabled,
                    }
                })
                .collect();
            tracing::debug!(
                "Reloaded {} password rules from app state",
                self.password_rules_dialog.rules.len()
            );
        }
        self.last_visited_page = Some(page.clone());

        let mut action = None;
        let mut navigate_to = None;

        let mut nav_target = None;
        let mut content_nav_target = None;
        let mut content_action = None;

        layout::render_settings_layout(
            ui,
            &shared.theme,
            |ui| {
                if let Some(target) = navigation::render_settings_navigator(ui, &shared.theme, page)
                {
                    nav_target = Some(crate::core::AppPage::Settings(target));
                }
            },
            |ui| {
                // Breadcrumb
                if let Some(target) = navigation::render_breadcrumb(ui, &shared.theme, &breadcrumb)
                {
                    content_nav_target = Some(target);
                }
                ui.add_space(8.0);

                // Header
                let header_action = header::render_header(ui, self, shared, page);

                // Handle SaveEditedRule immediately (before content rendering consumes rules_page)
                let mut rules_page = rules_page;
                if let Some(SettingsAction::SaveEditedRule) = &header_action {
                    if let Some(rp) = rules_page.as_mut() {
                        if let Some(org_service) = shared.services.organization_service.as_ref() {
                            match rp.save_editor_rule(org_service) {
                                Ok(()) => {
                                    rp.mark_saved_and_clear();
                                    content_nav_target = Some(crate::core::AppPage::Settings(SettingsPage::OrganizationRules));
                                }
                                Err(e) => {
                                    tracing::error!("Failed to save rule: {}", e);
                                }
                            }
                        }
                    }
                } else if let Some(act) = header_action {
                    content_action = Some(act);
                }

                ui.add_space(20.0);

                // Body
                egui::ScrollArea::vertical()
                    .id_salt("settings_content_scroll")
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        if !search_text.trim().is_empty() {
                            if let Some(target) = navigation::render_settings_search_results(
                                ui,
                                &shared.theme,
                                search_text,
                            ) {
                                content_nav_target = Some(crate::core::AppPage::Settings(target));
                            }
                        } else if *page == SettingsPage::Overview {
                            if let Some(target) =
                                navigation::render_settings_overview(ui, &shared.theme)
                            {
                                content_nav_target = Some(crate::core::AppPage::Settings(target));
                            }
                        } else {
                            let pm_arc_opt = shared.services.plugin_manager.clone();
                            let pm_guard = pm_arc_opt.as_ref().map(|m| m.lock());

                            let act = render_settings_content(
                                ui,
                                &shared.theme,
                                page,
                                &mut self.general_state,
                                &mut self.security_state,
                                &mut self.archives_state,
                                &mut self.password_rules_dialog,
                                pm_guard.as_deref(),
                                &mut self.plugins_state,
                                rules_page,
                                profiles_page,
                                &mut self.interface_state,
                                &mut self.toolbar_layout_state,
                                &mut self.info_panel_layout_state,
                                &mut self.keyboard_mouse_state,
                                &mut self.network_state,
                                &mut self.server_state,
                                &shared.app_state,
                                Some(shared),
                            );

                            if content_action.is_none() {
                                content_action = act;
                            }
                        }
                    });
            },
        );

        if let Some(t) = nav_target {
            navigate_to = Some(t);
        }
        if let Some(t) = content_nav_target {
            navigate_to = Some(t);
        }

        if let Some(act) = content_action {
            action = Some(act);
        }

        if let Some(action) = action {
            if let Some(target_page) =
                crate::features::settings::presentation::controllers::settings_controller::extract_navigation(&action)

            {
                navigate_to = Some(crate::core::AppPage::Settings(target_page));
            } else {
                self.handle_action(action, shared);
            }
        }

        navigate_to
    }

    pub fn handle_action(&mut self, action: SettingsAction, shared: &SharedState) {
        crate::features::settings::presentation::controllers::settings_controller::handle_action(
            action,
            &mut self.security_state,
            &mut self.archives_state,
            &mut self.plugins_state,
            &mut self.network_state,
            &mut self.server_state,
            shared,
        );
    }
}
