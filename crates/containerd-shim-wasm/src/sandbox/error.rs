use anyhow::Error as AnyError;
use containerd_shim::Error as ShimError;
use oci_spec::OciSpecError;
use thiserror::Error;
use ttrpc;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Oci(#[from] OciSpecError),
    #[error("{0}")]
    Stdio(#[from] std::io::Error),
    #[error("{0}")]
    Others(String),
    #[error("{0}")]
    Shim(#[from] ShimError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("{0}")]
    Any(#[from] AnyError),
    #[error("{0}")]
    FailedPrecondition(String),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Errno(#[from] nix::errno::Errno),
}

pub type Result<T> = ::std::result::Result<T, Error>;

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
            Error::Oci(ref s) => match s {
                _ => {
                    ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::UNKNOWN, e.to_string()))
                }
            },
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
                assert_eq!(s.code, ttrpc::Code::INVALID_ARGUMENT);
                assert_eq!(s.message, "invalid argument");
            }
            _ => panic!("unexpected error"),
        }

        let e = Error::NotFound("not found".to_string());
        let t: ttrpc::Error = e.into();
        match t {
            ttrpc::Error::RpcStatus(s) => {
                assert_eq!(s.code, ttrpc::Code::NOT_FOUND);
                assert_eq!(s.message, "not found");
            }
            _ => panic!("unexpected error"),
        }

        let e = Error::AlreadyExists("already exists".to_string());
        let t: ttrpc::Error = e.into();
        match t {
            ttrpc::Error::RpcStatus(s) => {
                assert_eq!(s.code, ttrpc::Code::ALREADY_EXISTS);
                assert_eq!(s.message, "already exists");
            }
            _ => panic!("unexpected error"),
        }

        let e = Error::FailedPrecondition("failed precondition".to_string());
        let t: ttrpc::Error = e.into();
        match t {
            ttrpc::Error::RpcStatus(s) => {
                assert_eq!(s.code, ttrpc::Code::FAILED_PRECONDITION);
                assert_eq!(s.message, "failed precondition");
            }
            _ => panic!("unexpected error"),
        }

        let e = Error::Shim(ShimError::InvalidArgument("invalid argument".to_string()));
        let t: ttrpc::Error = e.into();
        match t {
            ttrpc::Error::RpcStatus(s) => {
                assert_eq!(s.code, ttrpc::Code::INVALID_ARGUMENT);
                assert_eq!(s.message, "invalid argument");
            }
            _ => panic!("unexpected error"),
        }

        let e = Error::Any(AnyError::new(TestError::AnError("any error".to_string())));
        let t: ttrpc::Error = e.into();
        match t {
            ttrpc::Error::RpcStatus(s) => {
                assert_eq!(s.code, ttrpc::Code::UNKNOWN);
                assert_eq!(s.message, "any error");
            }
            _ => panic!("unexpected error"),
        }
    }
}
