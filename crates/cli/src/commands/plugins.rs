//! `arclain-cli plugins list [--json]` / `plugins enable ID` /
//! `plugins disable ID` / `plugins action ID NODE_ID --value-json JSON`

use arclain_app::event::OperationResult;
use arclain_app::plugins::{
    PluginActionDto, PluginActionRequest, PluginExtensionPointDto, PluginHostIntentDto,
    PluginUiNodeDto, PluginUiNodeKind, PluginUiUpdate,
};
use arclain_app::ArclainApp;
use clap::{Args, Subcommand};

use crate::output::{
    exit_code, exit_code_for, print_error, print_json, print_json_line, print_plain_error,
};

#[derive(Debug, Subcommand)]
pub enum PluginsCommand {
    /// List every plugin the application's plugin runtime knows about.
    List,
    /// Enable a plugin. **Architectural note**: this toggle lives only in
    /// the in-memory `PluginManager` of *this* process -- `ArclainApp::
    /// set_plugin_enabled` forwards to `arclain_plugins::PluginManager::
    /// enable_plugin`, which is a plain `RwLock<HashMap<...>>` write with
    /// no disk persistence underneath it at all. Since every CLI
    /// invocation bootstraps a brand-new process (and therefore a
    /// brand-new `PluginManager`), running this command has no effect
    /// observable from any later invocation -- see
    /// `crate::commands::plugins`'s own test
    /// `plugins_disable_has_no_effect_observable_from_a_separate_invocation`,
    /// which pins this as a characterized, real limitation rather than a
    /// CLI bug.
    Enable(PluginIdArgs),
    /// Disable a plugin. See [`PluginsCommand::Enable`]'s own doc
    /// comment -- the same in-memory-only limitation applies here.
    Disable(PluginIdArgs),
    /// Dispatch one interaction against a node of a plugin's main page.
    Action(ActionArgs),
}

#[derive(Debug, Args)]
pub struct PluginIdArgs {
    /// The plugin's id.
    pub id: String,
}

#[derive(Debug, Args)]
pub struct ActionArgs {
    /// The plugin's id.
    pub id: String,
    /// The target node's id, as it appears in the plugin's main-page
    /// document.
    pub node_id: String,
    /// A JSON value to submit as this node's new value: a JSON bool for
    /// a checkbox, a JSON number for a slider, or a JSON string for a
    /// text input / radio group / dropdown / tab selection. Omit
    /// entirely for a button- or list-item-style activation. Validated
    /// against the target node's own kind before anything is dispatched
    /// -- never accepted as a plain, unparsed command-line value: this
    /// command never reads a *password* this way (there is no `--value-json`
    /// use for a `Password` challenge, which this command never raises),
    /// and its own diagnostics never echo the raw value back, so a value
    /// happening to carry sensitive text is never repeated into this
    /// process's own output.
    #[arg(long = "value-json")]
    pub value_json: Option<String>,
}

pub async fn dispatch(app: &ArclainApp, command: &PluginsCommand, ctx: &super::Invocation) -> i32 {
    match command {
        PluginsCommand::List => run_list(app, ctx.json).await,
        PluginsCommand::Enable(args) => run_set_enabled(app, &args.id, true, ctx.json).await,
        PluginsCommand::Disable(args) => run_set_enabled(app, &args.id, false, ctx.json).await,
        PluginsCommand::Action(args) => run_action(app, args, ctx).await,
    }
}

async fn run_list(app: &ArclainApp, json: bool) -> i32 {
    let plugins = match app.plugins().await {
        Ok(plugins) => plugins,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            return code;
        }
    };

    if json {
        print_json(&plugins);
    } else if plugins.is_empty() {
        println!("(no plugins found)");
    } else {
        for plugin in &plugins {
            match &plugin.load_error {
                Some(reason) => println!("{}  (failed to load: {reason})", plugin.id),
                None => println!(
                    "{}  {}  {}  {}",
                    plugin.id,
                    plugin.name,
                    plugin.version,
                    if plugin.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                ),
            }
        }
    }
    exit_code::SUCCESS
}

async fn run_set_enabled(app: &ArclainApp, id: &str, enabled: bool, json: bool) -> i32 {
    match app.set_plugin_enabled(id.to_string(), enabled).await {
        Ok(()) => {
            if json {
                print_json(&crate::output::MutationOutcome::completed(None));
            } else {
                println!("{id}: {}", if enabled { "enabled" } else { "disabled" });
            }
            exit_code::SUCCESS
        }
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            code
        }
    }
}

/// Opens a `MainPage` session, validates and dispatches one action
/// against `args.node_id`, waits for `PluginUiUpdated`, then always
/// closes the session (best-effort) before returning.
async fn run_action(app: &ArclainApp, args: &ActionArgs, ctx: &super::Invocation) -> i32 {
    let snapshot = match app
        .open_plugin_session(args.id.clone(), PluginExtensionPointDto::MainPage)
        .await
    {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            return code;
        }
    };

    let action = match build_action(
        &snapshot.document.root,
        &args.node_id,
        args.value_json.as_deref(),
    ) {
        Ok(action) => action,
        Err(message) => {
            print_plain_error(&message);
            let _ = app.close_plugin_session(snapshot.session_id).await;
            return exit_code::UNSUPPORTED_INPUT;
        }
    };

    let mut events = app.subscribe_operations();
    let operation_id = match app
        .start_plugin_action(PluginActionRequest {
            session_id: snapshot.session_id,
            node_id: args.node_id.clone(),
            action,
        })
        .await
    {
        Ok(operation_id) => operation_id,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            let _ = app.close_plugin_session(snapshot.session_id).await;
            return code;
        }
    };

    let interactive = crate::events::std_interactive();
    let result = crate::events::drive_operation(
        crate::events::OperationWait {
            app,
            events: &mut events,
            operation_id,
            interactive: &interactive,
            cancel: &ctx.cancel,
            budget: ctx.budget,
        },
        ctx.json,
        |_event| {},
    )
    .await;

    let _ = app.close_plugin_session(snapshot.session_id).await;

    match result {
        Ok(OperationResult::PluginUiUpdated { update }) => {
            print_plugin_ui_update(&update, ctx.json);
            exit_code::SUCCESS
        }
        Ok(_) => {
            print_plain_error("unexpected result for a plugin action");
            exit_code::INTERNAL_FAILURE
        }
        Err(code) => code,
    }
}

/// Prints a successful action's result. Human mode stays deliberately
/// terse (the dispatched-action confirmation plus every host intent,
/// e.g. a toast message) rather than dumping the full updated node
/// tree -- this schema has no dedicated "secret" node kind (the closest
/// a plugin can get to a password field is a plain `TextInput`), so the
/// full tree could otherwise echo back exactly whatever free text a
/// caller just set via `--value-json`. `--json` mode is the documented,
/// explicit machine-payload path and *does* include the full
/// `PluginUiUpdate` (document and intents) verbatim, matching every
/// other command's own `--json` contract of reporting the facade's real
/// return value in full -- a caller who wants to avoid that can simply
/// not pass `--json`.
fn print_plugin_ui_update(update: &PluginUiUpdate, json: bool) {
    if json {
        // `print_json_line`, not `print_json`: `run_action` already
        // streamed `drive_operation`'s own JSON Lines events to stdout
        // before this prints -- see `print_json_line`'s own doc comment
        // for why the final envelope must stay a single compact line to
        // keep that whole stream parseable line by line.
        print_json_line(update);
        return;
    }
    for intent in &update.intents {
        print_intent_human(intent);
    }
    println!("action dispatched");
}

fn print_intent_human(intent: &PluginHostIntentDto) {
    match intent {
        PluginHostIntentDto::ShowToast { message, level } => {
            println!("toast ({level:?}): {message}");
        }
        PluginHostIntentDto::CloseDialog => println!("intent: close dialog"),
        PluginHostIntentDto::CopyToClipboard { .. } => println!("intent: copy to clipboard"),
        PluginHostIntentDto::OpenLightbox { .. } => println!("intent: open lightbox"),
        PluginHostIntentDto::SetPageDisplayName { name } => {
            println!("intent: set page display name to {name:?}");
        }
    }
}

/// Resolves `node_id` against `root`, validates `value_json` (if any)
/// against the node's own kind, and builds the [`PluginActionDto`] to
/// dispatch.
///
/// Rejects, with a plain human-readable message (never returned as an
/// [`arclain_app::error::ApplicationError`]: this is a purely local,
/// no-facade-call check, matching `crate::commands::list`'s own local
/// `ArchivePath::parse` validation):
/// - an id naming no node in the document (`root.find` returns `None`);
/// - a node that is hidden or disabled (mirrors the facade's own
///   `dispatch_action` gate -- see `arclain_app::plugins`'s module doc
///   comment -- but checked here too so an unusable target is rejected
///   before ever starting an operation, not after);
/// - a value shape that does not match the target node's kind (a
///   non-bool for a checkbox, an out-of-range or non-numeric value for a
///   slider, a string outside the configured option set for a radio
///   group/dropdown/tab bar);
/// - a value supplied for a kind that takes none (`Button`/`ListItem`),
///   or none supplied for a kind that requires one;
/// - any other node kind (a label, container, image, ...), which is not
///   directly actionable through this command at all.
///
/// Deliberately never echoes the raw `value_json` text back in any
/// message it builds (see [`print_plugin_ui_update`]'s own doc comment
/// for why): every message names the node id, the node's kind, and (for
/// a closed option set) the *valid* choices, never the caller's own
/// submitted value.
fn build_action(
    root: &PluginUiNodeDto,
    node_id: &str,
    value_json: Option<&str>,
) -> Result<PluginActionDto, String> {
    let node = root
        .find(node_id)
        .ok_or_else(|| format!("no node named {node_id:?} exists in this plugin's main page"))?;
    if !node.visible || !node.enabled {
        return Err(format!("node {node_id:?} is hidden or disabled"));
    }

    let value = match value_json {
        Some(raw) => Some(
            serde_json::from_str::<serde_json::Value>(raw)
                .map_err(|error| format!("--value-json is not valid JSON: {error}"))?,
        ),
        None => None,
    };

    match (&node.kind, value) {
        (PluginUiNodeKind::Button { .. } | PluginUiNodeKind::ListItem { .. }, None) => {
            Ok(PluginActionDto::Activate)
        }
        (PluginUiNodeKind::Button { .. } | PluginUiNodeKind::ListItem { .. }, Some(_)) => {
            Err(format!("node {node_id:?} does not accept --value-json"))
        }
        (PluginUiNodeKind::Checkbox { .. }, Some(serde_json::Value::Bool(checked))) => {
            Ok(set_value(checked.to_string()))
        }
        (PluginUiNodeKind::Checkbox { .. }, _) => Err(format!(
            "node {node_id:?} is a checkbox and needs a boolean --value-json (true or false)"
        )),
        (PluginUiNodeKind::TextInput { .. }, Some(serde_json::Value::String(text))) => {
            Ok(set_value(text))
        }
        (PluginUiNodeKind::TextInput { .. }, _) => Err(format!(
            "node {node_id:?} is a text input and needs a string --value-json"
        )),
        (
            PluginUiNodeKind::RadioGroup { options, .. },
            Some(serde_json::Value::String(selected)),
        ) => select_from_options(node_id, "radio group", options, selected),
        (PluginUiNodeKind::RadioGroup { .. }, _) => Err(format!(
            "node {node_id:?} is a radio group and needs a string --value-json naming one of its \
             options"
        )),
        (PluginUiNodeKind::Dropdown { options, .. }, Some(serde_json::Value::String(selected))) => {
            select_from_options(node_id, "dropdown", options, selected)
        }
        (PluginUiNodeKind::Dropdown { .. }, _) => Err(format!(
            "node {node_id:?} is a dropdown and needs a string --value-json naming one of its \
             options"
        )),
        (PluginUiNodeKind::Tabs { tabs, .. }, Some(serde_json::Value::String(selected))) => {
            select_from_options(node_id, "tab bar", tabs, selected)
        }
        (PluginUiNodeKind::Tabs { .. }, _) => Err(format!(
            "node {node_id:?} is a tab bar and needs a string --value-json naming one of its tabs"
        )),
        (PluginUiNodeKind::Slider { min, max, .. }, Some(value)) => {
            let number = value.as_f64().ok_or_else(|| {
                format!("node {node_id:?} is a slider and needs a numeric --value-json")
            })?;
            if number < f64::from(*min) || number > f64::from(*max) {
                Err(format!(
                    "node {node_id:?} only accepts a value between {min} and {max}"
                ))
            } else {
                Ok(set_value(number.to_string()))
            }
        }
        (PluginUiNodeKind::Slider { .. }, None) => Err(format!(
            "node {node_id:?} is a slider and requires --value-json"
        )),
        (other, _) => Err(format!(
            "node {node_id:?} ({}) is not directly actionable through this command",
            node_kind_label(other)
        )),
    }
}

fn set_value(value: String) -> PluginActionDto {
    PluginActionDto::SetValue { value: Some(value) }
}

fn select_from_options(
    node_id: &str,
    kind_label: &str,
    options: &[String],
    selected: String,
) -> Result<PluginActionDto, String> {
    if options.iter().any(|option| option == &selected) {
        Ok(set_value(selected))
    } else {
        Err(format!(
            "node {node_id:?} is a {kind_label} and only accepts one of {options:?}"
        ))
    }
}

/// A short, stable label for a node kind this command rejects as "not
/// directly actionable" -- deliberately not `{:?}` on the whole
/// [`PluginUiNodeKind`] value, which would dump every field (including,
/// for a container kind, its entire child subtree).
fn node_kind_label(kind: &PluginUiNodeKind) -> &'static str {
    match kind {
        PluginUiNodeKind::Single { .. } => "a single-column container",
        PluginUiNodeKind::Split { .. } => "a split container",
        PluginUiNodeKind::Label { .. } => "a label",
        PluginUiNodeKind::SectionHeader { .. } => "a section header",
        PluginUiNodeKind::Button { .. } => "a button",
        PluginUiNodeKind::TextInput { .. } => "a text input",
        PluginUiNodeKind::Checkbox { .. } => "a checkbox",
        PluginUiNodeKind::RadioGroup { .. } => "a radio group",
        PluginUiNodeKind::Slider { .. } => "a slider",
        PluginUiNodeKind::Dropdown { .. } => "a dropdown",
        PluginUiNodeKind::Image { .. } => "an image",
        PluginUiNodeKind::Separator => "a separator",
        PluginUiNodeKind::Space { .. } => "spacing",
        PluginUiNodeKind::Tabs { .. } => "a tab bar",
        PluginUiNodeKind::ListItem { .. } => "a list item",
        PluginUiNodeKind::ListContainer { .. } => "a list container",
        PluginUiNodeKind::Loading { .. } => "a loading indicator",
        PluginUiNodeKind::Group { .. } => "a group",
        PluginUiNodeKind::Warning { .. } => "a warning banner",
        PluginUiNodeKind::TagChips { .. } => "a tag chip list",
        PluginUiNodeKind::Toolbar { .. } => "a toolbar",
        PluginUiNodeKind::Carousel { .. } => "a carousel",
        PluginUiNodeKind::KeyValueList { .. } => "a key/value list",
        PluginUiNodeKind::MetadataGrid { .. } => "a metadata grid",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: PluginUiNodeKind) -> PluginUiNodeDto {
        PluginUiNodeDto {
            id: id.to_string(),
            kind,
            visible: true,
            enabled: true,
        }
    }

    fn root_with(children: Vec<PluginUiNodeDto>) -> PluginUiNodeDto {
        node("#root", PluginUiNodeKind::Single { children })
    }

    #[test]
    fn unknown_node_id_is_rejected() {
        let root = root_with(vec![]);
        let error = build_action(&root, "does-not-exist", None).unwrap_err();
        assert!(error.contains("does-not-exist"));
    }

    #[test]
    fn disabled_node_is_rejected() {
        let mut disabled = node(
            "btn",
            PluginUiNodeKind::Button {
                label: "Go".to_string(),
                action: None,
            },
        );
        disabled.enabled = false;
        let root = root_with(vec![disabled]);
        let error = build_action(&root, "btn", None).unwrap_err();
        assert!(error.contains("disabled"));
    }

    #[test]
    fn hidden_node_is_rejected() {
        let mut hidden = node(
            "btn",
            PluginUiNodeKind::Button {
                label: "Go".to_string(),
                action: None,
            },
        );
        hidden.visible = false;
        let root = root_with(vec![hidden]);
        let error = build_action(&root, "btn", None).unwrap_err();
        assert!(error.contains("hidden"));
    }

    #[test]
    fn button_with_no_value_activates() {
        let root = root_with(vec![node(
            "btn",
            PluginUiNodeKind::Button {
                label: "Go".to_string(),
                action: None,
            },
        )]);
        assert_eq!(
            build_action(&root, "btn", None).unwrap(),
            PluginActionDto::Activate
        );
    }

    #[test]
    fn button_rejects_a_supplied_value() {
        let root = root_with(vec![node(
            "btn",
            PluginUiNodeKind::Button {
                label: "Go".to_string(),
                action: None,
            },
        )]);
        let error = build_action(&root, "btn", Some("true")).unwrap_err();
        assert!(error.contains("does not accept"));
    }

    #[test]
    fn list_item_with_no_value_activates() {
        let root = root_with(vec![node(
            "row",
            PluginUiNodeKind::ListItem {
                title: "Row".to_string(),
                subtitle: None,
                badge: None,
                image_key: None,
                image_url: None,
                selected: false,
                warning_icon: None,
            },
        )]);
        assert_eq!(
            build_action(&root, "row", None).unwrap(),
            PluginActionDto::Activate
        );
    }

    #[test]
    fn checkbox_accepts_a_json_bool() {
        let root = root_with(vec![node(
            "check",
            PluginUiNodeKind::Checkbox {
                label: "Enable".to_string(),
                checked: false,
            },
        )]);
        assert_eq!(
            build_action(&root, "check", Some("true")).unwrap(),
            PluginActionDto::SetValue {
                value: Some("true".to_string())
            }
        );
    }

    #[test]
    fn checkbox_rejects_a_non_bool_value() {
        let root = root_with(vec![node(
            "check",
            PluginUiNodeKind::Checkbox {
                label: "Enable".to_string(),
                checked: false,
            },
        )]);
        let error = build_action(&root, "check", Some("\"topsecret\"")).unwrap_err();
        assert!(error.contains("boolean"));
        assert!(
            !error.contains("topsecret"),
            "the rejected value must never be echoed back: {error}"
        );
    }

    #[test]
    fn checkbox_requires_a_value() {
        let root = root_with(vec![node(
            "check",
            PluginUiNodeKind::Checkbox {
                label: "Enable".to_string(),
                checked: false,
            },
        )]);
        assert!(build_action(&root, "check", None).is_err());
    }

    #[test]
    fn text_input_accepts_a_json_string_and_never_echoes_it_on_error() {
        let root = root_with(vec![node(
            "field",
            PluginUiNodeKind::TextInput {
                label: "API Key".to_string(),
                value: String::new(),
                placeholder: None,
            },
        )]);
        assert_eq!(
            build_action(&root, "field", Some("\"correct horse battery staple\"")).unwrap(),
            PluginActionDto::SetValue {
                value: Some("correct horse battery staple".to_string())
            }
        );

        let error = build_action(&root, "field", Some("42")).unwrap_err();
        assert!(
            !error.contains("42"),
            "error must not echo the value: {error}"
        );
    }

    #[test]
    fn radio_group_accepts_a_listed_option_and_rejects_an_unlisted_one_without_echoing_it() {
        let root = root_with(vec![node(
            "theme",
            PluginUiNodeKind::RadioGroup {
                label: "Theme".to_string(),
                options: vec!["Light".to_string(), "Dark".to_string()],
                selected: "Light".to_string(),
            },
        )]);
        assert_eq!(
            build_action(&root, "theme", Some("\"Dark\"")).unwrap(),
            PluginActionDto::SetValue {
                value: Some("Dark".to_string())
            }
        );

        let error = build_action(&root, "theme", Some("\"Rainbow\"")).unwrap_err();
        assert!(error.contains("Light"));
        assert!(error.contains("Dark"));
        assert!(
            !error.contains("Rainbow"),
            "the rejected value must never be echoed back: {error}"
        );
    }

    #[test]
    fn dropdown_accepts_a_listed_option() {
        let root = root_with(vec![node(
            "mode",
            PluginUiNodeKind::Dropdown {
                label: "Mode".to_string(),
                options: vec!["Simple".to_string(), "Advanced".to_string()],
                selected: "Simple".to_string(),
            },
        )]);
        assert_eq!(
            build_action(&root, "mode", Some("\"Advanced\"")).unwrap(),
            PluginActionDto::SetValue {
                value: Some("Advanced".to_string())
            }
        );
        assert!(build_action(&root, "mode", Some("\"Unknown\"")).is_err());
    }

    #[test]
    fn tabs_accepts_a_listed_tab() {
        let root = root_with(vec![node(
            "tabs",
            PluginUiNodeKind::Tabs {
                tabs: vec!["One".to_string(), "Two".to_string()],
                selected: "One".to_string(),
            },
        )]);
        assert_eq!(
            build_action(&root, "tabs", Some("\"Two\"")).unwrap(),
            PluginActionDto::SetValue {
                value: Some("Two".to_string())
            }
        );
        assert!(build_action(&root, "tabs", Some("\"Three\"")).is_err());
    }

    #[test]
    fn slider_accepts_a_value_within_range_and_rejects_out_of_range() {
        let root = root_with(vec![node(
            "opacity",
            PluginUiNodeKind::Slider {
                label: "Opacity".to_string(),
                value: 0.5,
                min: 0.0,
                max: 1.0,
                step: Some(0.1),
            },
        )]);
        assert_eq!(
            build_action(&root, "opacity", Some("0.7")).unwrap(),
            PluginActionDto::SetValue {
                value: Some(0.7_f64.to_string())
            }
        );
        assert!(build_action(&root, "opacity", Some("1.5")).is_err());
        assert!(build_action(&root, "opacity", Some("-0.1")).is_err());
    }

    #[test]
    fn slider_rejects_a_non_numeric_value_and_requires_one() {
        let root = root_with(vec![node(
            "opacity",
            PluginUiNodeKind::Slider {
                label: "Opacity".to_string(),
                value: 0.5,
                min: 0.0,
                max: 1.0,
                step: None,
            },
        )]);
        assert!(build_action(&root, "opacity", Some("\"not-a-number\"")).is_err());
        assert!(build_action(&root, "opacity", None).is_err());
    }

    #[test]
    fn a_label_node_is_rejected_as_not_actionable() {
        let root = root_with(vec![node(
            "title",
            PluginUiNodeKind::Label {
                text: "Hello".to_string(),
                bold: false,
                size: None,
            },
        )]);
        let error = build_action(&root, "title", None).unwrap_err();
        assert!(error.contains("not directly actionable"));
    }

    #[test]
    fn malformed_json_value_is_rejected_without_a_panic() {
        let root = root_with(vec![node(
            "check",
            PluginUiNodeKind::Checkbox {
                label: "Enable".to_string(),
                checked: false,
            },
        )]);
        let error = build_action(&root, "check", Some("not json")).unwrap_err();
        assert!(error.contains("not valid JSON"));
    }
}
