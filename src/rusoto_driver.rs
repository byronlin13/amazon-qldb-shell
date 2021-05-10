use crate::error;
use crate::settings::Environment;
use amazon_qldb_driver::QldbDriverBuilder;
use amazon_qldb_driver::{retry, QldbDriver};
use anyhow::Result;
use async_trait::async_trait;
use rusoto_core::{Client, HttpClient, Region, credential::{DefaultCredentialsProvider, ProfileProvider, ProvideAwsCredentials}};
use rusoto_qldb_session::QldbSessionClient;
use std::str::FromStr;
use url::Url;

pub async fn build_driver(
    client: QldbSessionClient,
    ledger: String,
) -> Result<QldbDriver<QldbSessionClient>> {
    // We disable transaction retries because they don't make sense. Users
    // are entering statements, so if the tx fails they actually have to
    // enter them again! We can't simply remember their inputs and try
    // again, as individual statements may be derived from values seen from
    // yet other statements.
    QldbDriverBuilder::new()
        .ledger_name(ledger)
        .transaction_retry_policy(retry::never())
        .build_with_client(client)
        .await
}

/// Tries to start a session on the given ledger (via `env`). Fails with a
/// `usage_error` otherwise.
///
/// If a connection is formed, the new session is discarded and the client is
/// returned. The cleanup is just good manners, but the client is important
/// because it means future commands can reuse that same initial connection,
/// credentials, etc.
pub(crate) async fn health_check_start_session(env: &Environment) -> Result<QldbSessionClient> {
    use rusoto_qldb_session::*;
    let session_client = build_rusoto_client(&env).await?;

    let session_token = session_client
        .send_command(SendCommandRequest {
            start_session: Some(StartSessionRequest {
                ledger_name: env.ledger().value,
            }),
            ..Default::default()
        })
        .await
        .map_err(|e| {
            error::usage_error(
                format!(
                    r#"Unable to connect to ledger `{}`.

Please check the following:

- That you have specified a ledger that exists and is active
- That the AWS region you are targeting is correct
- That your AWS credentials are setup
- That your AWS credentials grant access on this ledger

The following error chain may have more information:
"#,
                    env.ledger().value
                ),
                e,
            )
        })?
        .start_session
        .and_then(|s| s.session_token);

    // Try be a good citizen, but don't fail if the new session can't be
    // released.
    let _ = session_client
        .send_command(SendCommandRequest {
            session_token,
            end_session: Some(EndSessionRequest {}),
            ..Default::default()
        })
        .await;

    Ok(session_client)
}

async fn build_rusoto_client(env: &Environment) -> Result<QldbSessionClient> {
    let provider = profile_provider(&env)?;
    let region = env.region().value;
    let creds = match provider {
        Some(p) => CredentialProvider::Profile(p),
        None => CredentialProvider::Default(DefaultCredentialsProvider::new()?),
    };

    let mut hyper = HttpClient::new()?;
    hyper.local_agent(format!(
        "QLDB Driver for Rust v{}/QLDB Shell for Rust v{}",
        amazon_qldb_driver::version(),
        env!("CARGO_PKG_VERSION")
    ));

    let client = Client::new_with(creds, hyper);
    Ok(QldbSessionClient::new_with_client(client, region))
}

/// Required for static dispatch of [`QldbSessionClient::new_with`].
enum CredentialProvider {
    Profile(ProfileProvider),
    Default(DefaultCredentialsProvider),
}

#[async_trait]
impl ProvideAwsCredentials for CredentialProvider {
    async fn credentials(
        &self,
    ) -> Result<rusoto_core::credential::AwsCredentials, rusoto_core::credential::CredentialsError>
    {
        use CredentialProvider::*;
        match self {
            Profile(p) => p.credentials().await,
            Default(c) => c.credentials().await,
        }
    }
}

fn profile_provider(env: &Environment) -> Result<Option<ProfileProvider>> {
    let it = match env.profile().value {
        Some(p) => {
            let mut prof = ProfileProvider::new()
                .map_err(|e| error::usage_error("Unable to create profile provider", e))?;
            prof.set_profile(p);
            Some(prof)
        }
        None => None,
    };

    Ok(it)
}

// FIXME: Default region should consider what is set in the Profile.
pub fn rusoto_region<S>(user_specified: Option<S>, custom_endpoint: Option<Url>) -> Result<Region>
where
    S: Into<String>,
{
    // Strip a trailing slash, otherwise things go wrong in hyper. Specifically,
    // it makes a POST request that looks like this:
    //
    //     POST // HTTP/1.1
    let custom_endpoint = custom_endpoint.map(|url| {
        url.to_string().trim_matches(|c| c == '/').to_string()
    });

    let it = match (user_specified, custom_endpoint) {
        (Some(r), Some(e)) => Region::Custom {
            name: r.into(),
            endpoint: e.into(),
        },
        (Some(r), None) => parse_region(r.into())?,
        (None, Some(e)) => Region::Custom {
            name: Region::default().name().to_owned(),
            endpoint: e.into(),
        },
        (None, None) => Region::default(),
    };

    Ok(it)
}

pub fn parse_region(r: impl AsRef<str>) -> Result<Region> {
    Ok(match Region::from_str(r.as_ref()) {
        Ok(it) => it,
        Err(e) => Err(error::usage_error(
            format!("Invalid region {}", r.as_ref()),
            e,
        ))?,
    })
}
