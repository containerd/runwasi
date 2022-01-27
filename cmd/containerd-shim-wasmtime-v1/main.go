package main

import (
	"context"

	"github.com/containerd/containerd/runtime/v2/shim"
	"github.com/cpuguy83/runwasi/wasmtimeshim"
	_ "github.com/cpuguy83/runwasi/wasmtimeshim/plugin"
	"github.com/moby/sys/mount"
)

func main() {
	if err := mount.MakeRSlave("/"); err != nil {
		panic(err)
	}
	shim.RunManager(context.Background(), wasmtimeshim.New("io.containerd.wasmtime.v1"))
}
