pub(crate) use roomkit_tools_http::{
    NetworkPolicy, checked_client as client, io_error as io, normalize_policy as normalize,
    parse_url as parse,
};

/// The host's origins, as the shared HTTP checks take them.
pub(crate) fn policy(policy: crate::WebFetchPolicy) -> NetworkPolicy {
    NetworkPolicy {
        allowed_origins: policy.allowed_origins,
        private_origins: policy.private_origins,
    }
}
