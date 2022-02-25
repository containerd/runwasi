use containerd_shim as shim;
use containerd_shim_wasmtimer_v1::Local;

fn main()  {
    shim::run::<Local>("io.containerd.empty.v1");
}