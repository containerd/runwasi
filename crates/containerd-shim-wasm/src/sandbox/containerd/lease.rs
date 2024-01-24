#![cfg(unix)]

use containerd_client::services::v1::leases_client::LeasesClient;
use containerd_client::{tonic, with_namespace};
use tonic::Request;

// Adds lease info to grpc header
// https://github.com/containerd/containerd/blob/8459273f806e068e1a6bacfaf1355bbbad738d5e/docs/garbage-collection.md#using-grpc
#[macro_export]
macro_rules! with_lease {
    ($req : ident, $ns: expr, $lease_id: expr) => {{
        let mut req = Request::new($req);
        let md = req.metadata_mut();
        // https://github.com/containerd/containerd/blob/main/namespaces/grpc.go#L27
        md.insert("containerd-namespace", $ns.parse().unwrap());
        md.insert("containerd-lease", $lease_id.parse().unwrap());
        req
    }};
}

#[derive(Debug)]
pub(crate) struct LeaseGuard {
    pub(crate) lease_id: String,
    pub(crate) namespace: String,
    pub(crate) address: String,
}

// Provides a best effort for dropping a lease of the content.  If the lease cannot be dropped, it will log a warning
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let id = self.lease_id.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let client = rt.block_on(containerd_client::connect(self.address.clone()));

        let channel = match client {
            Ok(channel) => channel,
            Err(e) => {
                log::error!(
                    "failed to connect to containerd: {}. lease may not be deleted",
                    e
                );
                return;
            }
        };

        let mut client = LeasesClient::new(channel);

        rt.block_on(async {
            let req = containerd_client::services::v1::DeleteRequest { id, sync: false };
            let req = with_namespace!(req, self.namespace);
            let result = client.delete(req).await;

            match result {
                Ok(_) => log::debug!("removed lease"),
                Err(e) => log::error!("failed to remove lease: {}", e),
            }
        });
    }
}
