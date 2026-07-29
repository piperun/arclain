use crate::core::SettingsPage;
use crate::features::settings::pages::interface::InterfaceSettingsState;
use crate::features::settings::pages::{InfoPanelLayoutState, ToolbarLayoutState};
use crate::features::settings::presentation::views::settings_content::{
    render_settings_content, ArchivesSettingsState, GeneralSettingsState, NetworkSettingsState,
    SecuritySettingsState, ServerSettingsState, SettingsAction, SettingsContentBorrows,
};

use crate::features::settings::views::{header, layout, navigation};
use crate::shared::SharedState;
use arclain_app::Signal;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};

/// Mutable borrows from sibling features that the settings router needs
/// at render time. Bundled into one parameter to keep
/// `SettingsFeature::render` from drowning in positional args — each
/// field is borrowed from its owning feature on `ArclainApp`.
pub struct SettingsFeatureBorrows<'a> {
    pub rules_page: Option<&'a mut crate::features::organization::presentation::views::RulesPage>,
    pub profiles_page:
        Option<&'a mut crate::features::organization::presentation::views::ProfilesPage>,
    pub hotkeys: Option<&'a mut crate::features::hotkeys::HotkeysFeature>,
    pub password_management:
        Option<&'a mut crate::features::password_management::PasswordManagementFeature>,
    pub plugins: Option<&'a mut crate::features::plugins::PluginsFeature>,
}

/// Immutable counterpart of [`SettingsFeatureBorrows`] used by
/// [`SettingsFeature::check_changes`]. Mirrors the same shape so call
/// sites can build it inline without juggling Option<&_> manually.
pub struct SettingsFeatureRefs<'a> {
    pub password_management:
        Option<&'a crate::features::password_management::PasswordManagementFeature>,
}

pub struct SettingsFeature {
    pub general_state: GeneralSettingsState,
    pub network_state: NetworkSettingsState,
    pub server_state: ServerSettingsState,
    pub security_state: SecuritySettingsState,
    pub archives_state: ArchivesSettingsState,

    pub interface_state: InterfaceSettingsState,
    pub toolbar_layout_state: ToolbarLayoutState,
    pub info_panel_layout_state: InfoPanelLayoutState,

    /// Cached dirty state for rule editor (synced from RulesPage each frame)
    pub rule_editor_dirty: bool,
    /// Whether connection_test_status signal has been bound to egui context
    signals_bound: AtomicBool,
}

impl SettingsFeature {
    pub fn new(shared: &SharedState) -> Self {
        // Seed the form state from the settings mirrors.
        let general = shared.signals().general_settings.get();
        let network = shared.signals().network_settings.get();
        let open_nested_in_new_tab = general.open_nested_in_new_tab;
        let drop_behavior = crate::features::settings::types::DropBehavior::from_settings_str(
            &general.drop_behavior,
        );
        let restore_tabs_on_launch = general.restore_tabs_on_launch;

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

            use arclain_app::Signal;

            NetworkSettingsState {
                socks5_enabled: Signal::new(network.socks5_enabled),
                socks5_address: Signal::new(network.socks5_address.clone().unwrap_or_default()),
                socks5_username: Signal::new(network.socks5_username.clone().unwrap_or_default()),
                socks5_password: Signal::new(password),
                connection_test_status: Signal::new(ConnectionTestStatus::Idle),
            }
        };

        let server_state = {
            use crate::features::settings::domain::types::ServerConnectionStatus;
            use arclain_app::Signal;

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
                enabled: Signal::new(network.gameta_server_enabled),
                url: Signal::new(network.gameta_server_url.clone().unwrap_or_default()),
                api_key: Signal::new(api_key),
                connection_status: Signal::new(ServerConnectionStatus::Idle),
            }
        };

        Self {
            general_state: GeneralSettingsState {
                open_nested_in_new_tab: Signal::new(open_nested_in_new_tab),
                drop_behavior: Signal::new(drop_behavior),
                restore_tabs_on_launch: Signal::new(restore_tabs_on_launch),
            },
            network_state,
            server_state,
            security_state: {
                let security = shared.signals().security_settings.get();
                SecuritySettingsState {
                    default_secrets_db: security.default_secrets_database_path,
                    default_key_file: security.default_key_file_path,
                    ..SecuritySettingsState::default()
                }
            },
            archives_state: ArchivesSettingsState::default(),

            interface_state: InterfaceSettingsState::default(),
            toolbar_layout_state: ToolbarLayoutState::default(),
            info_panel_layout_state: InfoPanelLayoutState::default(),
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
        self.network_state
            .connection_test_status
            .subscribe(move || {
                ctx_network.request_repaint();
            });
        let ctx_server = ctx.clone();
        self.server_state.connection_status.subscribe(move || {
            ctx_server.request_repaint();
        });
    }

    pub fn check_changes(
        &self,
        shared: &SharedState,
        page: &SettingsPage,
        refs: SettingsFeatureRefs<'_>,
    ) -> bool {
        // PasswordRules dirty-detection is owned by PasswordManagementFeature
        // — short-circuit before locking app_state for the other arms.
        if matches!(page, SettingsPage::PasswordRules) {
            return refs
                .password_management
                .map(|pm| pm.is_dirty(shared))
                .unwrap_or(false);
        }

        let state = shared.app_state.lock();

        match page {
            SettingsPage::General => {
                let stored_drop = crate::features::settings::types::DropBehavior::from_settings_str(
                    state
                        .user_config
                        .drop_behavior
                        .as_deref()
                        .unwrap_or("new_tab"),
                );
                *self.general_state.open_nested_in_new_tab.read()
                    != state.user_config.open_nested_in_new_tab
                    || *self.general_state.drop_behavior.read() != stored_drop
                    || *self.general_state.restore_tabs_on_launch.read()
                        != state.user_config.restore_tabs_on_launch
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
            _ => false,
        }
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        shared: &SharedState,
        page: &SettingsPage,
        breadcrumb: Vec<(String, crate::core::AppPage)>,
        borrows: SettingsFeatureBorrows<'_>,
        search_text: &str,
    ) -> Option<crate::core::AppPage> {
        let SettingsFeatureBorrows {
            mut rules_page,
            profiles_page,
            mut hotkeys,
            mut password_management,
            mut plugins,
        } = borrows;

        // Bind signals to egui context (once)
        self.bind_signals(ui.ctx());

        // Sync rule editor dirty state for header
        self.rule_editor_dirty = rules_page
            .as_ref()
            .map(|rp| rp.is_editor_dirty())
            .unwrap_or(false);

        if let Some(pm) = password_management.as_deref_mut() {
            pm.sync_on_page_change(shared, page);
        }

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

                // Header — needs read-only hotkeys for SaveKeyboardMouse action,
                // password_management for SavePasswordRules / dirty check, and
                // mutable plugins (settings page list_state) for the Plugins
                // page header dispatch.
                let header_action = header::render_header(
                    ui,
                    self,
                    hotkeys.as_deref(),
                    password_management.as_deref(),
                    plugins.as_deref_mut().map(|p| &mut p.settings_list_state),
                    shared,
                    page,
                );

                // Handle SaveEditedRule immediately (before content rendering consumes rules_page)
                if let Some(SettingsAction::SaveEditedRule) = &header_action {
                    if let Some(rp) = rules_page.as_mut() {
                        if let Some(org_service) = shared.services.organization_service.as_ref() {
                            match rp.save_editor_rule(org_service) {
                                Ok(()) => {
                                    rp.mark_saved_and_clear();
                                    content_nav_target = Some(crate::core::AppPage::Settings(
                                        SettingsPage::OrganizationRules,
                                    ));
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
                            let content_borrows = SettingsContentBorrows {
                                general: &mut self.general_state,
                                security: &mut self.security_state,
                                archives: &mut self.archives_state,
                                network: &mut self.network_state,
                                server: &mut self.server_state,
                                interface: &mut self.interface_state,
                                toolbar_layout: &mut self.toolbar_layout_state,
                                info_panel_layout: &mut self.info_panel_layout_state,
                                password_rules_dialog: password_management
                                    .as_deref_mut()
                                    .map(|pm| &mut pm.password_rules_dialog),
                                plugins_state: plugins
                                    .as_deref_mut()
                                    .map(|p| &mut p.settings_list_state),
                                keyboard_mouse_state: hotkeys
                                    .as_deref_mut()
                                    .map(|h| &mut h.keyboard_mouse_state),
                                rules_page,
                                profiles_page,
                            };
                            let act = render_settings_content(
                                ui,
                                &shared.theme,
                                page,
                                content_borrows,
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
                self.handle_action(
                    action,
                    shared,
                    plugins.as_mut().map(|p| &mut p.settings_list_state),
                );
            }
        }

        navigate_to
    }

    pub fn handle_action(
        &mut self,
        action: SettingsAction,
        shared: &SharedState,
        plugins_state: Option<&mut crate::features::plugins::domain::types::PluginsListState>,
    ) {
        crate::features::settings::presentation::controllers::settings_controller::handle_action(
            action,
            &mut self.security_state,
            &mut self.archives_state,
            plugins_state,
            &mut self.network_state,
            &mut self.server_state,
            shared,
        );
    }
}
