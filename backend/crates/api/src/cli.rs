//! Command line of the `api` binary.

/// What the binary is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Serve the API (default).
    Serve,
    /// Apply pending database migrations, then exit.
    Migrate,
    /// Print the OpenAPI contract, then exit.
    OpenApi,
    /// Probe the running server's liveness endpoint, or its readiness one
    /// (container health check).
    Healthcheck { ready: bool },
    /// Print usage, then exit.
    Help,
}

pub const USAGE: &str = "\
Usage: api [COMMAND]

Commands:
  serve     Serve the REST API (default)
  migrate   Apply pending database migrations with DATABASE_MIGRATION_URL, then exit
  openapi   Print the OpenAPI contract (JSON) on standard output, then exit
  healthcheck [--ready]
            Exit 0 if the server listening on API_BIND_ADDR is alive (or, with
            --ready, can serve: database and shared services reachable)
  help      Print this message

Configuration is read from environment variables; see deploy/.env.example.";

/// Parses the arguments that follow the program name.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let args: Vec<String> = args.into_iter().collect();
    match args.as_slice() {
        [] => Ok(Command::Serve),
        [one] => match one.as_str() {
            "serve" => Ok(Command::Serve),
            "migrate" => Ok(Command::Migrate),
            "openapi" => Ok(Command::OpenApi),
            "healthcheck" => Ok(Command::Healthcheck { ready: false }),
            "help" | "--help" | "-h" => Ok(Command::Help),
            other => Err(format!("unknown command: {other}")),
        },
        [first, flag] if first == "healthcheck" && flag == "--ready" => {
            Ok(Command::Healthcheck { ready: true })
        }
        _ => Err(format!("unexpected arguments: {}", args[1..].join(" "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn no_argument_serves() {
        assert_eq!(parse(args(&[])), Ok(Command::Serve));
        assert_eq!(parse(args(&["serve"])), Ok(Command::Serve));
    }

    #[test]
    fn migrate_openapi_and_help_are_recognized() {
        assert_eq!(parse(args(&["migrate"])), Ok(Command::Migrate));
        assert_eq!(parse(args(&["openapi"])), Ok(Command::OpenApi));
        assert_eq!(
            parse(args(&["healthcheck"])),
            Ok(Command::Healthcheck { ready: false })
        );
        assert_eq!(
            parse(args(&["healthcheck", "--ready"])),
            Ok(Command::Healthcheck { ready: true })
        );
        assert!(parse(args(&["healthcheck", "--now"])).is_err());
        for flag in ["help", "--help", "-h"] {
            assert_eq!(parse(args(&[flag])), Ok(Command::Help));
        }
    }

    #[test]
    fn unknown_or_extra_arguments_are_rejected() {
        assert_eq!(
            parse(args(&["migrat"])),
            Err("unknown command: migrat".to_owned())
        );
        assert_eq!(
            parse(args(&["migrate", "now"])),
            Err("unexpected arguments: now".to_owned())
        );
    }
}
