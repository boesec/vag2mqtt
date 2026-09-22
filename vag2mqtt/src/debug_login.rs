//! The `debug-login` subcommand: one login attempt, then exit.
//!
//! The WP-06 login spike. It answers one question, on real hardware: does a login against this
//! account work today, and if not, at which step and with which answer does it break.
//!
//! It touches neither the database nor MQTT nor the supervisor, and it makes exactly one attempt
//! per invocation, so it cannot walk an account into `login.error.throttled`. The password is
//! read from `VAG2MQTT_DEBUG_PASSWORD` or prompted for; it is never an argument, because
//! arguments show up in the process list and in shell history.

use anyhow::{Context, Result, bail};
use vag2mqtt_connector_api::{Connector, ConnectorError, Credentials};
use vag2mqtt_connector_audi::{AudiConnector, TraceConfig};
use vag2mqtt_domain::Secret;

use crate::cli::{DebugBrand, DebugLogin};

/// The environment variable the password may come from.
const PASSWORD_ENV: &str = "VAG2MQTT_DEBUG_PASSWORD";

/// Runs one login attempt. Returns `Ok(())` when a session was obtained.
pub(crate) async fn run(args: DebugLogin) -> Result<()> {
    let password = read_password()?;
    let trace = match &args.trace_dir {
        Some(dir) => TraceConfig::into_directory(dir),
        None => TraceConfig::from_env(),
    };

    let credentials = Credentials {
        username: args.username.clone(),
        password,
    };

    let connector = match args.brand {
        DebugBrand::Audi => {
            AudiConnector::new(trace.clone()).map_err(|e| anyhow::anyhow!("{e}"))?
        }
    };

    println!("vag2mqtt debug-login");
    println!("  brand      {:?}", args.brand);
    println!("  strategy   {}", connector.strategy_name());
    println!(
        "  trace      {}",
        if trace.is_enabled() {
            "on"
        } else {
            "off (set --trace-dir or VAG2MQTT_AUDI_TRACE to record)"
        }
    );
    println!("  attempts   exactly one");
    println!();

    let outcome = connector.login(&credentials).await;
    report(&outcome);
    match outcome {
        Ok(_) => Ok(()),
        Err(error) => bail!("login failed: {error}"),
    }
}

/// Prints the outcome. Tokens are never printed, only their lifetime.
fn report(outcome: &Result<vag2mqtt_connector_api::SessionState, ConnectorError>) {
    match outcome {
        Ok(session) => {
            println!("RESULT  success");
            match session.expires_at {
                Some(expiry) => {
                    let lifetime = expiry - chrono::Utc::now();
                    println!(
                        "  the access token is usable for another {} minutes (refresh due {})",
                        lifetime.num_minutes().max(0),
                        expiry.to_rfc3339()
                    );
                }
                None => println!("  the backend named no expiry"),
            }
            println!("  no token is printed; it stays in memory and is discarded on exit");
        }
        Err(error) => {
            println!("RESULT  failure");
            println!("  error    {error}");
            println!("  class    {:?}", error.class());
            println!("  meaning  {}", meaning(error));
        }
    }
}

/// What the failure means for the project, in one line.
fn meaning(error: &ConnectorError) -> &'static str {
    match error {
        ConnectorError::InvalidCredentials => {
            "the manufacturer rejected user name or password; check them before retrying"
        }
        ConnectorError::TwoFactorRequired => {
            "the account asks for a second factor; the form login cannot serve it, the device code route is the answer"
        }
        ConnectorError::CaptchaRequired => {
            "the login presented a CAPTCHA; the form login cannot serve it, the device code route is the answer"
        }
        ConnectorError::RateLimited { .. } => {
            "the account is throttled; wait before trying again, repeated attempts risk a lockout"
        }
        ConnectorError::Network { .. } => {
            "the manufacturer could not be reached; a transport problem, not a flow problem"
        }
        ConnectorError::Manufacturer {
            status: Some(400), ..
        } => {
            "the backend refused the exchange; this is the shape of reference issue #32, and the trace files say which field it dislikes"
        }
        ConnectorError::Manufacturer { .. } => {
            "the backend answered with an error of its own; transient unless it repeats"
        }
        ConnectorError::Parsing { context } => match *context {
            "authorize" | "signin_form" => {
                "the sign-in page no longer looks like the scraped form; this is the shape of reference issue #34"
            }
            _ => {
                "the flow reached an answer it could not read; the trace files show the step and the page"
            }
        },
        ConnectorError::SessionExpired => "the session was rejected; a full login is needed",
        _ => "see the trace files for the step and the response",
    }
}

/// Reads the password from the environment or prompts for it. Never an argument.
fn read_password() -> Result<Secret<String>> {
    if let Ok(password) = std::env::var(PASSWORD_ENV) {
        if password.is_empty() {
            bail!("{PASSWORD_ENV} is set but empty");
        }
        return Ok(Secret::new(password));
    }
    let password = rpassword::prompt_password("Password (not echoed): ")
        .with_context(|| format!("no password given; set {PASSWORD_ENV} or type one"))?;
    if password.is_empty() {
        bail!("no password given");
    }
    Ok(Secret::new(password))
}

#[cfg(test)]
mod tests {
    use vag2mqtt_connector_audi::TRACE_ENV;

    use super::*;

    #[test]
    fn every_error_has_a_meaning_and_none_leaks() {
        let errors = [
            ConnectorError::InvalidCredentials,
            ConnectorError::TwoFactorRequired,
            ConnectorError::CaptchaRequired,
            ConnectorError::RateLimited { retry_after: None },
            ConnectorError::network("timeout"),
            ConnectorError::Manufacturer {
                status: Some(400),
                code: Some("invalid_request".into()),
            },
            ConnectorError::Manufacturer {
                status: Some(503),
                code: None,
            },
            ConnectorError::Parsing {
                context: "signin_form",
            },
            ConnectorError::Parsing {
                context: "redirect_chase",
            },
            ConnectorError::SessionExpired,
            ConnectorError::VehicleNotFound,
        ];
        for error in errors {
            let text = meaning(&error);
            assert!(!text.is_empty(), "{error} has no meaning");
            assert!(text.len() > 20, "{error}: {text}");
        }
    }

    #[test]
    fn the_trace_variable_is_the_connector_one() {
        assert_eq!(TRACE_ENV, "VAG2MQTT_AUDI_TRACE");
    }
}
