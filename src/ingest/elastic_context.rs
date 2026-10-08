use std::fmt;
use std::sync::Arc;

use elasticrc::{ConfigFile, ContextServiceReference};
use elasticsearch::Elasticsearch;

use super::source::ElasticContextTarget;

pub(crate) struct ContextConnection {
    pub(crate) client: Arc<Elasticsearch>,
    pub(crate) safe_identity: Arc<str>,
}

impl fmt::Debug for ContextConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextConnection")
            .field("source", &self.safe_identity)
            .finish_non_exhaustive()
    }
}

pub(crate) fn resolve_connection(
    target: &ElasticContextTarget,
) -> anyhow::Result<Arc<ContextConnection>> {
    match target.connection_cache().get_or_init(|| {
        resolve_selected_connection(target)
            .map(Arc::new)
            .map_err(|error| error.to_string())
    }) {
        Ok(connection) => Ok(connection.clone()),
        Err(message) => Err(anyhow::anyhow!(message.clone())),
    }
}

fn resolve_selected_connection(target: &ElasticContextTarget) -> anyhow::Result<ContextConnection> {
    target.validate()?;
    let identity = target.safe_identity();
    let Some(ContextServiceReference::Elasticsearch { context }) =
        ContextServiceReference::parse(target.reference())
    else {
        anyhow::bail!("invalid Elasticsearch context reference");
    };
    let config = ConfigFile::load_with_options(None, None)
        .map_err(|error| safe_resolution_error(error, &identity))?;
    let context_name = context
        .as_deref()
        .unwrap_or_else(|| config.current_context_name());
    let selected = config
        .context(context_name)
        .map_err(|error| safe_resolution_error(error, &identity))?;
    let service = selected.elasticsearch.as_ref().ok_or_else(|| {
        anyhow::anyhow!("{identity}: selected context has no Elasticsearch service")
    })?;
    // Resolving this service, rather than the entire configuration, keeps every
    // unselected service's credential and URL resolvers inert.
    let resolved = service
        .resolve()
        .map_err(|error| safe_resolution_error(error, &identity))?;
    let client = super::elasticsearch::build_context_client(&resolved.url, resolved.auth)
        .map_err(|error| anyhow::anyhow!("{identity}: {error}"))?;
    Ok(ContextConnection {
        client: Arc::new(client),
        safe_identity: identity.into(),
    })
}

fn safe_resolution_error(error: elasticrc::Error, identity: &str) -> anyhow::Error {
    // Dependency errors may contain endpoints, inline values, command output,
    // or resolver expressions. Never retain them as an anyhow source chain.
    let stage = match error {
        elasticrc::Error::ConfigNotFound { .. } => {
            "no Elastic CLI configuration found; set ELASTIC_CLI_CONFIG_FILE or create a home .elasticrc"
        }
        elasticrc::Error::HomeDirectoryUnavailable => {
            "home directory unavailable; set ELASTIC_CLI_CONFIG_FILE"
        }
        elasticrc::Error::ExecutableConfigUnsupported { .. } => {
            "executable Elastic CLI configuration is unsupported; use JSON or YAML"
        }
        elasticrc::Error::Io { .. } => {
            "cannot read Elastic CLI configuration; check ELASTIC_CLI_CONFIG_FILE and file permissions"
        }
        elasticrc::Error::Json { .. } | elasticrc::Error::Yaml { .. } => {
            "cannot parse Elastic CLI configuration; check its JSON or YAML syntax"
        }
        elasticrc::Error::InvalidShape(_) => {
            "invalid Elastic CLI configuration; check current_context and contexts"
        }
        elasticrc::Error::MissingContext { .. } => {
            "selected Elastic CLI context was not found; check the reference and current_context"
        }
        elasticrc::Error::MissingService { .. } => {
            "selected context has no Elasticsearch service"
        }
        elasticrc::Error::InvalidServiceUrl { .. } => {
            "selected Elasticsearch endpoint is not a valid HTTP(S) URL"
        }
        elasticrc::Error::InvalidServiceUrlScheme { .. } => {
            "selected Elasticsearch endpoint must use HTTP(S)"
        }
        elasticrc::Error::InvalidAuth { .. } => {
            "selected Elasticsearch authentication is invalid; configure API-key, basic, or no authentication"
        }
        elasticrc::Error::InvalidResolverExpression { .. }
        | elasticrc::Error::UnknownResolver { .. }
        | elasticrc::Error::ShellSyntaxUnsupported { .. } => {
            "selected Elasticsearch service contains an invalid or unsupported resolver"
        }
        elasticrc::Error::ResolverFailed { .. } => {
            "selected Elasticsearch service resolver failed; check the trusted resolver and its inputs"
        }
    };
    anyhow::anyhow!("{identity}: {stage}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_connection_debug_retains_safe_identity_without_transport_secrets() {
        let sentinel = "context-secret-sentinel";
        let endpoint =
            url::Url::parse(&format!("http://elastic.example:9200/?key={sentinel}")).unwrap();
        let client = super::super::elasticsearch::build_context_client(
            &endpoint,
            elasticrc::Auth::api_key(sentinel),
        )
        .unwrap();
        let connection = ContextConnection {
            client: Arc::new(client),
            safe_identity: ".elasticsearch://logs-*".into(),
        };
        let debug = format!("{connection:?}");
        assert!(debug.contains(".elasticsearch://logs-*"));
        assert!(!debug.contains(sentinel));
        assert!(!debug.contains("elastic.example"));
    }

    #[test]
    fn dependency_failures_do_not_retain_secret_values_or_sources() {
        let sentinel = "context-secret-sentinel";
        let errors = [
            elasticrc::Error::ResolverFailed {
                resolver: sentinel.to_owned(),
                field: sentinel.to_owned(),
                message: sentinel.to_owned(),
            },
            elasticrc::Error::InvalidResolverExpression {
                field: sentinel.to_owned(),
                value: sentinel.to_owned(),
            },
            elasticrc::Error::InvalidServiceUrlScheme {
                context: Some(sentinel.to_owned()),
                service: "elasticsearch",
                value: format!("file:///{sentinel}"),
            },
            elasticrc::Error::MissingContext {
                name: sentinel.to_owned(),
                available: vec![sentinel.to_owned()],
            },
        ];
        for error in errors {
            let safe = safe_resolution_error(error, ".production.elasticsearch://logs-*");
            assert!(!format!("{safe:#}").contains(sentinel));
            assert!(!format!("{safe:?}").contains(sentinel));
            assert!(safe.source().is_none());
        }
    }
}
