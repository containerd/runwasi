package wasmtimeshim

import (
	"context"
	"net"

	"github.com/bytecodealliance/wasmtime-go"
	"github.com/containerd/containerd/runtime/v2/task"
	"github.com/containerd/ttrpc"
)

type Sandbox struct {
	srv *ttrpc.Server

	engine *wasmtime.Engine
	linker *wasmtime.Linker
	store  *wasmtime.Store

	instances *instanceStore
}

func NewSandbox() (*Sandbox, error) {
	cfg := wasmtime.NewConfig()
	cfg.SetInterruptable(true)

	engine := wasmtime.NewEngineWithConfig(cfg)
	linker := wasmtime.NewLinker(engine)
	store := wasmtime.NewStore(engine)

	if err := linker.DefineWasi(); err != nil {
		return nil, err
	}

	srv, err := ttrpc.NewServer()
	if err != nil {
		return nil, err
	}

	s := &Sandbox{
		engine:    engine,
		linker:    linker,
		store:     store,
		instances: newInstanceStore(),
		srv:       srv,
	}

	srv.Register("io.containerd.wasmtime.v1.runtime", s.serviceMethods())

	return s, nil
}

func (s *Sandbox) StartService(ctx context.Context, l net.Listener) error {
	if err := s.srv.Serve(ctx, l); err != nil {
		return err
	}

	return nil
}

func (s *Sandbox) serviceMethods() map[string]ttrpc.Method {
	return map[string]ttrpc.Method{
		"Create": s.Create,
	}
}

func (s *Service) getSandboxClient(ctx context.Context) (task.TaskService, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.sandboxClient != nil {
		return s.sandboxClient, nil
	}

	ch := make(chan task.TaskService)

	go func() {
		for s.sandboxClient == nil {
			if err := ctx.Err(); err != nil {
				return
			}
			s.cond.Wait()
		}
		ch <- s.sandboxClient
	}()

	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case client := <-ch:
		return client, nil
	}
}
