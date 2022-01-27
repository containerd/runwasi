runwasi is an attempt to create a runc-compatible binary to run wasm workloads.
Upon working on this I decided it was easier to do what I wanted at the containerd-shim layer.
It may still be worthwhile to do something at this layer (though there are some other projects which do this to an extent already).