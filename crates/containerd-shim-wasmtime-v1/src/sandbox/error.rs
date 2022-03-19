use super::oci;
use anyhow::Error as AnyError;
use containerd_shim::Error as ShimError;
use thiserror::Error;
use ttrpc;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Oci(#[from] oci::Error),
    #[error("{0}")]
    Stdio(#[from] std::io::Error),
    #[error("{0}")]
    Others(String),
    #[error("{0}")]
    Wasi(#[from] wasmtime_wasi::Error),
    #[error("{0}")]
    WasiCommonStringArray(#[from] wasi_common::StringArrayError),
    #[error("{0}")]
    Shim(#[from] ShimError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("{0}")]
    Any(AnyError),
    #[error("{0}")]
    FailedPrecondition(String),
}

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
                oci::Error::InvalidArgument(s) => {
                    ttrpc::Error::RpcStatus(ttrpc::get_status(ttrpc::Code::INVALID_ARGUMENT, s))
                }
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
