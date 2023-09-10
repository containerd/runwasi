pub struct TestModule {
    pub source: &'static str,
    pub bytes: &'static [u8],
}

impl AsRef<[u8]> for TestModule {
    fn as_ref(&self) -> &[u8] {
        self.bytes
    }
}

include!(concat!(env!("OUT_DIR"), "/modules.rs"));
