package wasmtimeshim

import (
	"context"
	"fmt"
	"os"
	"time"

	"github.com/bytecodealliance/wasmtime-go"
	taskapi "github.com/containerd/containerd/api/types/task"
	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/runtime/v2/task"
)

func (s *Service) Start(ctx context.Context, req *task.StartRequest) (_ *task.StartResponse, retErr error) {
	defer func() {
		if retErr != nil {
			retErr = fmt.Errorf("start: %w", retErr)
		}
	}()

	if req.ExecID != "" {
		return nil, fmt.Errorf("exec: %w", errdefs.ErrNotImplemented)
	}

	s.mu.Lock()
	if s.sandboxID == req.ID {
		pid := s.pid
		s.mu.Unlock()
		return &task.StartResponse{Pid: pid}, nil
	}
	s.mu.Unlock()

	client, err := s.getSandboxClient(ctx)
	if err != nil {
		return nil, err
	}
	return client.Start(ctx, req)
}

func (s *Sandbox) Start(Ctx context.Context, req *task.StartRequest) (*task.StartResponse, error) {
	i := s.instances.Get(req.ID)
	if i == nil {
		return nil, errdefs.ErrNotFound
	}

	fn := i.i.GetExport(s.store, "_start").Func()
	if fn == nil {
		return nil, fmt.Errorf("%w: module start function not found", os.ErrNotExist)
	}

	go func() {
		_, err := fn.Call(s.store)
		i.mu.Lock()
		i.status = taskapi.StatusStopped
		i.exitedAt = time.Now()
		if err != nil {
			i.exitCode = uint32(*err.(*wasmtime.Trap).Code())
		}
		i.cond.Broadcast()
		i.mu.Unlock()
		i.cancel()

	}()

	return &task.StartResponse{Pid: i.pid}, nil
}
