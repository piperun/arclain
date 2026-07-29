//! `arclain-cli profiles list [--json]` / `arclain-cli profiles show ID [--json]`

use arclain_app::settings::OrganizationProfileSummary;
use arclain_app::ArclainApp;
use clap::Subcommand;

use crate::output::{exit_code, exit_code_for, print_error, print_json};

#[derive(Debug, Subcommand)]
pub enum ProfilesCommand {
    /// List every configured organization/output profile.
    List,
    /// Show one organization/output profile by id.
    Show {
        /// The profile id to show.
        id: String,
    },
}

pub async fn dispatch(app: &ArclainApp, command: &ProfilesCommand, json: bool) -> i32 {
    match command {
        ProfilesCommand::List => run_list(app, json).await,
        ProfilesCommand::Show { id } => run_show(app, id, json).await,
    }
}

async fn run_list(app: &ArclainApp, json: bool) -> i32 {
    let profiles = match app.organization_profiles().await {
        Ok(profiles) => profiles,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            return code;
        }
    };

    if json {
        print_json(&profiles);
    } else if profiles.is_empty() {
        println!("(no organization profiles configured)");
    } else {
        for profile in &profiles {
            println!(
                "{}  {}  ({})",
                profile.id, profile.name, profile.output_format
            );
        }
    }
    exit_code::SUCCESS
}

async fn run_show(app: &ArclainApp, id: &str, json: bool) -> i32 {
    let profiles = match app.organization_profiles().await {
        Ok(profiles) => profiles,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            return code;
        }
    };

    match find_profile(&profiles, id) {
        Some(profile) => {
            if json {
                print_json(profile);
            } else {
                println!("id: {}", profile.id);
                println!("name: {}", profile.name);
                println!("output_format: {}", profile.output_format);
            }
            exit_code::SUCCESS
        }
        None => {
            crate::output::print_plain_error(&format!("no such profile: {id}"));
            exit_code::UNSUPPORTED_INPUT
        }
    }
}

fn find_profile<'a>(
    profiles: &'a [OrganizationProfileSummary],
    id: &str,
) -> Option<&'a OrganizationProfileSummary> {
    profiles.iter().find(|profile| profile.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile whose only interesting fields are the two
    /// [`find_profile`] matches on; everything else is filler.
    fn profile(id: &str) -> OrganizationProfileSummary {
        OrganizationProfileSummary {
            id: id.to_string(),
            name: format!("Profile {id}"),
            description: None,
            output_format: "zip".to_string(),
            compression_level: 5,
            compression_method: None,
            solid_archive: false,
            encrypt_headers: false,
            is_default: false,
            is_system: false,
        }
    }

    #[test]
    fn find_profile_matches_by_exact_id() {
        let profiles = vec![profile("a"), profile("b")];
        assert_eq!(
            find_profile(&profiles, "b").map(|p| p.id.as_str()),
            Some("b")
        );
    }

    #[test]
    fn find_profile_returns_none_for_an_unknown_id() {
        let profiles = vec![profile("a")];
        assert!(find_profile(&profiles, "does-not-exist").is_none());
    }
}
