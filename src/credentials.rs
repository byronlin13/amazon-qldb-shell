use std::pin::Pin;

//use aws_auth::provider::AsyncProvideCredentials;
use aws_auth::provider::{CredentialsError, CredentialsResult};
use aws_sdk_qldbsession::Credentials;
use futures::Future;
use rusoto_core::credential::ProvideAwsCredentials;

pub(crate) fn from_rusoto<P: ProvideAwsCredentials>(rusoto: P) -> RusotoProvider<P> {
    RusotoProvider(rusoto)
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// An asynchronous credentials provider
///
/// If your use-case is synchronous, you should implement [ProvideCredentials] instead.
pub trait AsyncProvideCredentials: Send + Sync {
    fn provide_credentials<'a>(&'a self) -> BoxFuture<'a, CredentialsResult>;
}

pub(crate) struct RusotoProvider<P: ProvideAwsCredentials>(P);
impl<P> AsyncProvideCredentials for RusotoProvider<P>
where
    P: ProvideAwsCredentials + Send + Sync,
{
    fn provide_credentials(&self) -> BoxFuture<CredentialsResult> {
        Box::pin(async { map(self.0.credentials().await) })
    }
}

fn map(
    rusoto: Result<
        rusoto_core::credential::AwsCredentials,
        rusoto_core::credential::CredentialsError,
    >,
) -> CredentialsResult {
    match rusoto {
        Ok(credentials) => Ok(Credentials::from_keys(
            credentials.aws_access_key_id(),
            credentials.aws_secret_access_key(),
            credentials.token().to_owned(),
        )),
        Err(err) => Err(CredentialsError::Unhandled(Box::new(err))),
    }
}
