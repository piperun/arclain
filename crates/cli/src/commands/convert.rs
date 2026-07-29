//! `arclain-cli convert INPUT... --destination PATH --format FORMAT [--flatten]`

use std::path::PathBuf;

use arclain_app::operations::ConvertRequest;
use arclain_app::ArclainApp;
use clap::Args;

use crate::output::{
    exit_code, exit_code_for, print_error, print_json_line, print_plain_error, MutationOutcome,
};

#[derive(Debug, Args)]
pub struct ConvertArgs {
    /// Archive files to convert. Each keeps its own output name (by
    /// stem/detected metadata title).
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
    /// Directory to write the converted archives into.
    #[arg(long)]
    pub destination: PathBuf,
    /// Target archive format: "zip" or "7z" (also accepts "sevenz").
    #[arg(long)]
    pub format: String,
    /// Flattens nested archives before converting.
    #[arg(long)]
    pub flatten: bool,
}

/// Validates every input exists locally, starts conversion, drives it to
/// a terminal state. Returns the process exit code. No archive session
/// is opened -- `ConvertRequest` operates directly on filesystem paths.
pub async fn run(app: &ArclainApp, args: &ConvertArgs, json: bool) -> i32 {
    let inputs = match resolve_inputs(&args.inputs) {
        Ok(inputs) => inputs,
        Err(code) => return code,
    };
    let destination = match super::absolutize(&args.destination) {
        Ok(destination) => destination,
        Err(code) => return code,
    };

    let mut events = app.subscribe_operations();
    let operation_id = match app
        .start_convert(ConvertRequest {
            inputs,
            destination,
            format: args.format.clone(),
            flatten: args.flatten,
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
    let mut cancel = crate::events::CancelTrigger::CtrlC;
    let mut last_message = super::LastProgressMessage::default();
    let result = crate::events::drive_operation(
        app,
        &mut events,
        operation_id,
        json,
        &interactive,
        &mut cancel,
        |event| last_message.observe(event),
    )
    .await;

    match result {
        Ok(_) => {
            let summary = last_message.into_inner();
            if json {
                print_json_line(&MutationOutcome::completed(summary));
            } else {
                match &summary {
                    Some(summary) => println!("conversion complete: {summary}"),
                    None => println!("conversion complete"),
                }
            }
            exit_code::SUCCESS
        }
        Err(code) => code,
    }
}

/// Validates every one of `inputs` exists as a real file, then
/// absolutizes each -- shared by every batch-processing command
/// (`convert`, `organize`, `pipeline run`) whose inputs are real
/// filesystem paths rather than in-archive entries.
pub(crate) fn resolve_inputs(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, i32> {
    for input in inputs {
        if !input.is_file() {
            print_plain_error(&format!("input not found: {}", input.display()));
            return Err(exit_code::UNSUPPORTED_INPUT);
        }
    }
    let mut resolved = Vec::with_capacity(inputs.len());
    for input in inputs {
        resolved.push(super::absolutize(input)?);
    }
    Ok(resolved)
}
