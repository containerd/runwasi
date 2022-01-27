package wasmtimeoci

import (
	"context"
	"fmt"

	"github.com/bytecodealliance/wasmtime-go"
)

type Runtime struct {
	// Root is the path for the runtime to store runtime state
	// This should generally be in a tmpfs as the data should get cleaned up when the system restarts.
	Root string

	// wasmtime components needed to run oci bundles.
	Engine *wasmtime.Engine
	Linker *wasmtime.Linker
	Store  *wasmtime.Store
}

// NewRuntime creates a wasmtime based oci runtime that can execute oci bundles within a single wasmttime instance.
func NewRuntime(root string) (*Runtime, error) {
	engine := wasmtime.NewEngine()

	linker := wasmtime.NewLinker(engine)
	store := wasmtime.NewStore(engine)

	if err := linker.DefineWasi(); err != nil {
		return nil, err
	}

	return &Runtime{
		Root:   root,
		Engine: engine,
		Linker: linker,
		Store:  store,
	}, nil
}

func (r *Runtime) Create(ctx context.Context, bundle, pidFile string) error {
	return fmt.Errorf("not implemented")
}
