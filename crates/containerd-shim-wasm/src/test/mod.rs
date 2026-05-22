mod signals;

#[ctor::ctor(unsafe)]
fn init_zygote() {
    containerd_shimkit::zygote::Zygote::global();
}
