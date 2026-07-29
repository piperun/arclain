//! `arclain-cli organize INPUT... --destination PATH --profile ID --rule ID [--dry-run]`

use std::path::PathBuf;

use arclain_app::operations::OrganizeRequest;
use arclain_app::ArclainApp;
use clap::Args;

use crate::output::{exit_code, exit_code_for, print_error, print_json_line, MutationOutcome};

#[derive(Debug, Args)]
pub struct OrganizeArgs {
    /// Archive files to organize.
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
    /// Directory to write the organized archives into.
    #[arg(long)]
    pub destination: PathBuf,
    /// The output archive profile id -- governs format/compression. A
    /// separate id from `--rule`: see `arclain_app::operations::OrganizeRequest`'s
    /// own doc comment for why organization needs both.
    #[arg(long)]
    pub profile: String,
    /// The organization rule id -- governs the organized layout (which
    /// files go where).
    #[arg(long)]
    pub rule: String,
    /// Computes the plan without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

/// Validates every input exists locally, starts organizing, drives it to
/// a terminal state. Returns the process exit code. No archive session
/// is opened -- `OrganizeRequest` operates directly on filesystem paths.
pub async fn run(app: &ArclainApp, args: &OrganizeArgs, ctx: &super::Invocation) -> i32 {
    let inputs = match super::convert::resolve_inputs(&args.inputs) {
        Ok(inputs) => inputs,
        Err(code) => return code,
    };
    let destination = match super::absolutize(&args.destination) {
        Ok(destination) => destination,
        Err(code) => return code,
    };

    let mut events = app.subscribe_operations();
    let operation_id = match app
        .start_organize(OrganizeRequest {
            inputs,
            destination,
            profile_id: args.profile.clone(),
            rule_id: args.rule.clone(),
            dry_run: args.dry_run,
        })
        .await
    {
        Ok(operation_id) => operation_id,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            return code;
        }
    };

    let interactive = crate::events::std_interactive();
    let mut last_message = super::LastProgressMessage::default();
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
        |event| last_message.observe(event),
    )
    .await;

    match result {
        Ok(_) => {
            let summary = last_message.into_inner();
            if ctx.json {
                print_json_line(&MutationOutcome::completed(summary));
            } else {
                match &summary {
                    Some(summary) => println!("organization complete: {summary}"),
                    None => println!("organization complete"),
                }
            }
            exit_code::SUCCESS
        }
        Err(code) => code,
    }
}
