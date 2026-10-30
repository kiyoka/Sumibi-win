//! Native HTTPS preparation only. Sending still requires the ordered consent
//! gate and completion/cancellation processing in a future transport adapter.
use crate::manual::{Destination, Scheme};
use sumibi_winhttp::{Error, PreparedRequest};

pub fn prepare(destination: &Destination) -> Result<PreparedRequest, Error> {
    if destination.scheme() != Scheme::Https {
        return Err(Error::InvalidDestination);
    }
    PreparedRequest::new(
        destination.host(),
        destination.port(),
        destination.base_path(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_preparation_refuses_diagnostic_http() {
        let destination =
            Destination::new(Scheme::HttpLoopback, "127.0.0.1", 8080, "/convert").unwrap();
        assert!(matches!(
            prepare(&destination),
            Err(Error::InvalidDestination)
        ));
    }
}
