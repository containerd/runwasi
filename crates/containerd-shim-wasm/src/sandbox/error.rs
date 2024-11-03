//! Error types used by shims
//! This handles converting to the appropriate ttrpc error codes

use anyhow::Error as AnyError;
use containerd_shim::protos::ttrpc;
use containerd_shim::Error as ShimError;
use oci_spec::OciSpecError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    /// An error occurred while parsing the OCI spec
    #[error("{0}")]
    Oci(#[from] OciSpecError),
    /// An error that can occur while setting up the environment for the container
    #[error("{0}")]
    Stdio(#[from] std::io::Error),
    #[error("{0}")]
    Others(String),
    /// Errors to/from the containerd shim library.
    #[error("{0}")]
    Shim(#[from] ShimError),
    /// Requested item is not found
    #[error("not found: {0}")]
    NotFound(String),
    /// Requested item already exists
    #[error("already exists: {0}")]
    AlreadyExists(String),
    /// Supplied arguments/options/config is invalid
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Any other error
    #[error("{0}")]
    Any(#[from] AnyError),
    /// The operation was rejected because the system is not in a state required for the operation's
    #[error("{0}")]
    FailedPrecondition(String),
    /// Error while parsing JSON
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    /// Error from the system
    #[cfg(unix)]
    #[error("{0}")]
    Errno(#[from] nix::errno::Errno),
    /// Errors from libcontainer
    #[cfg(unix)]
    #[error("{0}")]
    Libcontainer(#[from] libcontainer::error::LibcontainerError),
    #[error("{0}")]
    Containerd(String),
}

pub type Result<T, E = Error> = ::std::result::Result<T, E>;

impl From<Error> for ttrpc::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Shim(ref s) => match s {
                ShimError::InvalidArgument(s) => {
                    ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::INVALID_ARGUMENT, s))
                }
                ShimError::NotFoundError(s) => {
                    ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::NOT_FOUND, s))
                }
                _ => ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::UNKNOWN, s)),
            },
            Error::NotFound(ref s) => {
                ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::NOT_FOUND, s))
            }
            Error::AlreadyExists(ref s) => {
                ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::ALREADY_EXISTS, s))
            }
            Error::InvalidArgument(ref s) => {
                ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::INVALID_ARGUMENT, s))
            }
            Error::FailedPrecondition(ref s) => {
                ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::FAILED_PRECONDITION, s))
            }
            Error::Oci(ref _s) => {
                ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::UNKNOWN, e.to_string()))
            }
            Error::Any(ref s) => {
                ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::UNKNOWN, s))
            }
            _ => ttrpc::Error::Others(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use thiserror::Error;

    use super::*;

    #[derive(Debug, Error)]
    enum TestError {
        #[error("{0}")]
        AnError(String),
    }

    #[test]
    fn test_error_to_ttrpc_status() {
        let e = Error::InvalidArgument("invalid argument".to_string());
        let t: ttrpc::Error = e.into();
        match t {
            ttrpc::Error::RpcStatus(s) => {
                assert_eq!(s.code(), ttrpc::Code::INVALID_ARGUMENT);
                assert_eq!(s.message, "invalid argument");
            }
            _ => panic!("unexpected error"),
        }

        let e = Error::NotFound("not found".to_string());
        let t: ttrpc::Error = e.into();
        match t {
            ttrpc::Error::RpcStatus(s) => {
                assert_eq!(s.code(), ttrpc::Code::NOT_FOUND);
                assert_eq!(s.message, "not found");
            }
            _ => panic!("unexpected error"),
        }

        let e = Error::AlreadyExists("already exists".to_string());
        let t: ttrpc::Error = e.into();
        match t {
            ttrpc::Error::RpcStatus(s) => {
                assert_eq!(s.code(), ttrpc::Code::ALREADY_EXISTS);
                assert_eq!(s.message, "already exists");
            }
            _ => panic!("unexpected error"),
        }

        let e = Error::FailedPrecondition("failed precondition".to_string());
        let t: ttrpc::Error = e.into();
        match t {
            ttrpc::Error::RpcStatus(s) => {
                assert_eq!(s.code(), ttrpc::Code::FAILED_PRECONDITION);
                assert_eq!(s.message, "failed precondition");
            }
            _ => panic!("unexpected error"),
        }

        let e = Error::Shim(ShimError::InvalidArgument("invalid argument".to_string()));
        let t: ttrpc::Error = e.into();
        match t {
            ttrpc::Error::RpcStatus(s) => {
                assert_eq!(s.code(), ttrpc::Code::INVALID_ARGUMENT);
                assert_eq!(s.message, "invalid argument");
            }
            _ => panic!("unexpected error"),
        }

        let e = Error::Any(AnyError::new(TestError::AnError("any error".to_string())));
        let t: ttrpc::Error = e.into();
        match t {
            ttrpc::Error::RpcStatus(s) => {
                assert_eq!(s.code(), ttrpc::Code::UNKNOWN);
                assert_eq!(s.message, "any error");
            }
            _ => panic!("unexpected error"),
        }
    }
}
