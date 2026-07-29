//! `arclain-cli pipeline run INPUT... --destination PATH|--same-folder --preset ID [--collision POLICY]`
//!
//! Only a saved preset can be run through this command
//! (`PipelineSpecDto::Preset`) -- an ad-hoc, hand-assembled step list
//! (`PipelineSpecDto::Steps`, the shape the Process page's own step
//! builder produces) is not exposed by this CLI. A step list has no
//! stable, guessable identity of its own to name on a command line the
//! way a saved preset's own name does, and building one would mean this
//! command growing its own flag surface for every current and future
//! `PipelineStepDto` variant; that is deliberately out of this task's
//! scope, not an oversight.

use std::path::PathBuf;

use arclain_app::operations::pipeline::{
    OutputCollisionPolicyDto, PipelineDestinationDto, PipelineSpecDto,
};
use arclain_app::operations::PipelineRequest;
use arclain_app::ArclainApp;
use clap::{Args, Subcommand, ValueEnum};

use crate::output::{
    exit_code, exit_code_for, print_error, print_json_line, print_plain_error, MutationOutcome,
};

#[derive(Debug, Subcommand)]
pub enum PipelineCommand {
    /// Runs a saved processing pipeline preset over a batch of inputs.
    Run(RunArgs),
}

/// Mirrors `arclain_app::operations::pipeline::OutputCollisionPolicyDto`'s
/// own `snake_case` spelling.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum CollisionPolicyArg {
    Fail,
    Skip,
    Overwrite,
    Smart,
}

impl CollisionPolicyArg {
    fn to_facade(self) -> OutputCollisionPolicyDto {
        match self {
            Self::Fail => OutputCollisionPolicyDto::Fail,
            Self::Skip => OutputCollisionPolicyDto::Skip,
            Self::Overwrite => OutputCollisionPolicyDto::Overwrite,
            Self::Smart => OutputCollisionPolicyDto::Smart,
        }
    }
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Archive files to run the pipeline over.
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
    /// Folder to write results into. Exactly one of `--destination`/
    /// `--same-folder` is required.
    #[arg(long)]
    pub destination: Option<PathBuf>,
    /// Writes each result alongside its own input file, instead of a
    /// separate destination folder.
    #[arg(long)]
    pub same_folder: bool,
    /// The saved preset's name to run.
    #[arg(long)]
    pub preset: String,
    /// Overrides the preset's own collision policy for this run.
    #[arg(long, value_enum)]
    pub collision: Option<CollisionPolicyArg>,
}

pub async fn dispatch(app: &ArclainApp, command: &PipelineCommand, ctx: &super::Invocation) -> i32 {
    match command {
        PipelineCommand::Run(args) => run(app, args, ctx).await,
    }
}

async fn run(app: &ArclainApp, args: &RunArgs, ctx: &super::Invocation) -> i32 {
    let destination = match resolve_destination(args) {
        Ok(destination) => destination,
        Err(code) => return code,
    };
    let inputs = match super::convert::resolve_inputs(&args.inputs) {
        Ok(inputs) => inputs,
        Err(code) => return code,
    };

    let mut events = app.subscribe_operations();
    let operation_id = match app
        .start_pipeline(PipelineRequest {
            inputs,
            destination,
            pipeline: PipelineSpecDto::Preset {
                id: args.preset.clone(),
            },
            collision_policy: args.collision.map(CollisionPolicyArg::to_facade),
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
                    Some(summary) => println!("pipeline run complete: {summary}"),
                    None => println!("pipeline run complete"),
                }
            }
            exit_code::SUCCESS
        }
        Err(code) => code,
    }
}

/// Resolves `--destination`/`--same-folder` into a `PipelineDestinationDto`.
/// Both fields are individually optional at the `clap` level (no
/// dependency on a `clap::ArgGroup`'s own derive attribute syntax);
/// "exactly one is required" is this function's own semantic check,
/// classified the same way `crate::commands::list`'s local `ArchivePath`
/// validation is: a purely local input-shape problem
/// (`exit_code::UNSUPPORTED_INPUT`), not a clap usage error.
fn resolve_destination(args: &RunArgs) -> Result<PipelineDestinationDto, i32> {
    match (&args.destination, args.same_folder) {
        (Some(path), false) => {
            super::absolutize(path).map(|path| PipelineDestinationDto::Folder { path })
        }
        (None, true) => Ok(PipelineDestinationDto::SameFolder),
        (None, false) => {
            print_plain_error("exactly one of --destination or --same-folder is required");
            Err(exit_code::UNSUPPORTED_INPUT)
        }
        (Some(_), true) => {
            print_plain_error("--destination and --same-folder cannot both be given");
            Err(exit_code::UNSUPPORTED_INPUT)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(inputs: Vec<PathBuf>) -> RunArgs {
        RunArgs {
            inputs,
            destination: None,
            same_folder: false,
            preset: "preset".to_string(),
            collision: None,
        }
    }

    #[test]
    fn neither_destination_nor_same_folder_is_rejected() {
        assert_eq!(
            resolve_destination(&args(vec![])),
            Err(exit_code::UNSUPPORTED_INPUT)
        );
    }

    #[test]
    fn both_destination_and_same_folder_is_rejected() {
        let mut request = args(vec![]);
        request.destination = Some(PathBuf::from("out"));
        request.same_folder = true;
        assert_eq!(
            resolve_destination(&request),
            Err(exit_code::UNSUPPORTED_INPUT)
        );
    }

    #[test]
    fn same_folder_alone_resolves_to_the_same_folder_variant() {
        let mut request = args(vec![]);
        request.same_folder = true;
        assert_eq!(
            resolve_destination(&request).unwrap(),
            PipelineDestinationDto::SameFolder
        );
    }

    #[test]
    fn destination_alone_resolves_to_an_absolute_folder() {
        let mut request = args(vec![]);
        request.destination = Some(PathBuf::from("relative-out"));
        let resolved = resolve_destination(&request).unwrap();
        match resolved {
            PipelineDestinationDto::Folder { path } => assert!(path.is_absolute()),
            other => panic!("expected Folder, got {other:?}"),
        }
    }

    #[test]
    fn collision_policy_arg_round_trips_to_the_facade_spelling() {
        assert_eq!(
            serde_json::to_value(CollisionPolicyArg::Fail.to_facade()).unwrap(),
            serde_json::json!("fail")
        );
        assert_eq!(
            serde_json::to_value(CollisionPolicyArg::Skip.to_facade()).unwrap(),
            serde_json::json!("skip")
        );
        assert_eq!(
            serde_json::to_value(CollisionPolicyArg::Overwrite.to_facade()).unwrap(),
            serde_json::json!("overwrite")
        );
        assert_eq!(
            serde_json::to_value(CollisionPolicyArg::Smart.to_facade()).unwrap(),
            serde_json::json!("smart")
        );
    }
}
