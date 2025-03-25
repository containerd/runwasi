mod signals;

#[ctor::ctor]
fn init_zygote() {
    containerd_shimkit::zygote::Zygote::global();
}
