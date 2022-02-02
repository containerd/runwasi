package main

/*
#cgo CFLAGS: -Wall
extern void hook();
void __attribute__((constructor)) init(void) {
	hook();
}
*/
import "C"

import (
	"context"
	"fmt"
	"net"
	"os"
	"os/signal"

	"github.com/containerd/containerd/runtime/v2/shim"
	"github.com/cpuguy83/runwasi/wasmtimeshim"
	_ "github.com/cpuguy83/runwasi/wasmtimeshim/plugin"
	"github.com/moby/sys/mount"
	"golang.org/x/sys/unix"
)

func main() {
	if os.Getenv("_RUNWASI_SANDBOX") == "1" {
		if err := runSandbox(); err != nil {
			fmt.Fprintln(os.Stderr, err.Error())
			os.Exit(1)
		}
		return
	}
	if err := mount.MakeRSlave("/"); err != nil {
		panic(err)
	}
	shim.RunManager(context.Background(), wasmtimeshim.New("io.containerd.wasmtime.v1"))
}

func runSandbox() error {
	ch := make(chan os.Signal, 1)
	signal.Notify(ch, os.Interrupt, unix.SIGTERM)
	defer signal.Stop(ch)
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		select {
		case <-ch:
			cancel()
		case <-ctx.Done():
		}
	}()

	s, err := wasmtimeshim.NewSandbox()
	if err != nil {
		return err
	}

	l, err := net.Listen("unix", "sandbox.sock")
	if err != nil {
		return fmt.Errorf("error listening on sandbox socket: %w", err)
	}
	defer l.Close()

	if err := s.StartService(ctx, l); err != nil {
		return err
	}

	return nil
}
