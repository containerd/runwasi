use chrono::{DateTime, TimeZone};
use containerd_shim::event::Event;
use containerd_shim::publisher::RemotePublisher;
use log::warn;
use protobuf::MessageDyn;
use protobuf::well_known_types::timestamp::Timestamp;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

pub trait EventSender: Clone + Send + Sync + 'static {
    fn send(&self, event: impl Event);
}

#[derive(Clone)]
pub struct RemoteEventSender {
    tx: UnboundedSender<(String, Box<dyn MessageDyn>)>,
}

impl RemoteEventSender {
    pub fn new(namespace: impl AsRef<str>, publisher: RemotePublisher) -> RemoteEventSender {
        let namespace = namespace.as_ref().to_string();
        let (tx, mut rx) = unbounded_channel::<(String, Box<dyn MessageDyn>)>();
        tokio::spawn(async move {
            while let Some((topic, event)) = rx.recv().await {
                if let Err(err) = publisher
                    .publish(Default::default(), &topic, &namespace, event)
                    .await
                {
                    warn!("failed to publish event, topic: {topic}: {err}")
                }
            }
        });
        RemoteEventSender { tx }
    }
}

impl EventSender for RemoteEventSender {
    fn send(&self, event: impl Event) {
        let topic = event.topic();
        let event = Box::new(event);
        if let Err(SendError((topic, _))) = self.tx.send((topic, event)) {
            warn!("failed to publish event, topic: {topic}: channel closed")
        }
    }
}

pub(super) trait ToTimestamp {
    fn to_timestamp(self) -> Timestamp;
}

impl<Tz: TimeZone> ToTimestamp for DateTime<Tz> {
    fn to_timestamp(self) -> Timestamp {
        Timestamp {
            seconds: self.timestamp(),
            nanos: self.timestamp_subsec_nanos() as i32,
            ..Default::default()
        }
    }
}
