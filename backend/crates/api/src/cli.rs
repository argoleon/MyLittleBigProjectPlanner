//! Command line of the `api` binary.

use domain::account::{normalize_email, validate_display_name, validate_password};
use infra::accounts::{AccountType, AddedMember, NewMember, TenantRole};
use infra::sqlx::postgres::PgPool;

/// What the binary is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Add an internal member to a tenant (password on standard input).
    AddMember(MemberArgs),
    /// Print usage, then exit.
    Help,
}

pub const USAGE: &str = "\
Usage: api [COMMAND]

Commands:
  serve     Serve the REST API (default)
  migrate   Apply pending database migrations with DATABASE_MIGRATION_URL, then
            create the first tenant and platform super admin if BOOTSTRAP_ADMIN_EMAIL
            is set and none exists yet; exit
  openapi   Print the OpenAPI contract (JSON) on standard output, then exit
  healthcheck [--ready]
            Exit 0 if the server listening on API_BIND_ADDR is alive (or, with
            --ready, can serve: database and shared services reachable)
  add-member TENANT EMAIL DISPLAY_NAME [--role member|configuration_admin|tenant_admin]
            Add an internal member to the tenant TENANT (its slug), with the
            password read from standard input (first line), then exit. An
            existing account is linked and keeps its password. Uses DATABASE_URL.
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
        [first, tenant, email, name, rest @ ..] if first == "add-member" => {
            let role = match rest {
                [] => TenantRole::Member,
                [flag, role] if flag == "--role" => {
                    TenantRole::parse(role).ok_or_else(|| format!("unknown role: {role}"))?
                }
                _ => return Err(format!("unexpected arguments: {}", rest.join(" "))),
            };
            Ok(Command::AddMember(MemberArgs {
                tenant: tenant.clone(),
                email: email.clone(),
                display_name: name.clone(),
                role,
            }))
        }
        _ => Err(format!("unexpected arguments: {}", args[1..].join(" "))),
    }
}

/// Arguments of `add-member`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberArgs {
    /// Slug of the tenant.
    pub tenant: String,
    pub email: String,
    pub display_name: String,
    pub role: TenantRole,
}

/// Adds the member of `args` with `password`. Errors are messages for the
/// operator.
pub async fn add_member(pool: &PgPool, args: &MemberArgs, password: &str) -> Result<AddedMember, String> {
    let email = normalize_email(&args.email).ok_or_else(|| format!("invalid email: {}", args.email))?;
    let display_name = args.display_name.trim();
    validate_display_name("display name", display_name)
        .map_err(|e| format!("display name: {}", e[0].code))?;
    validate_password("password", password, &email).map_err(|e| format!("password: {}", e[0].code))?;
    let hash = infra::credentials::hash_password(password).map_err(|e| e.to_string())?;

    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let tenant = infra::db::resolve_tenant(&mut tx, &args.tenant)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no tenant with the slug {}", args.tenant))?;
    infra::db::set_tenant(&mut tx, tenant.id)
        .await
        .map_err(|e| e.to_string())?;
    let added = infra::accounts::add_member(
        &mut tx,
        &NewMember {
            email: &email,
            display_name,
            password_hash: Some(&hash),
            account_type: AccountType::Internal,
            tenant_role: args.role,
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(added)
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
    fn add_member_takes_a_tenant_an_email_a_name_and_an_optional_role() {
        assert_eq!(
            parse(args(&["add-member", "acme", "lucas@acme.io", "Lucas Bernard"])),
            Ok(Command::AddMember(MemberArgs {
                tenant: "acme".to_owned(),
                email: "lucas@acme.io".to_owned(),
                display_name: "Lucas Bernard".to_owned(),
                role: TenantRole::Member,
            }))
        );
        let admin = parse(args(&[
            "add-member",
            "acme",
            "a@acme.io",
            "A",
            "--role",
            "tenant_admin",
        ]));
        assert!(matches!(
            admin,
            Ok(Command::AddMember(MemberArgs {
                role: TenantRole::TenantAdmin,
                ..
            }))
        ));
        assert_eq!(
            parse(args(&["add-member", "acme", "a@acme.io", "A", "--role", "owner"])),
            Err("unknown role: owner".to_owned())
        );
        assert!(parse(args(&["add-member", "acme", "a@acme.io"])).is_err());
        assert!(parse(args(&["add-member", "acme", "a@acme.io", "A", "--admin"])).is_err());
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
