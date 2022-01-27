package wasmtimeshim

import (
	"context"
	"fmt"

	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/runtime/v2/task"
)

func (s *Service) State(ctx context.Context, req *task.StateRequest) (*task.StateResponse, error) {
	if req.ExecID != "" {
		return nil, fmt.Errorf("exec: %w", errdefs.ErrNotImplemented)
	}

	i := s.instances.Get(req.ID)
	if i == nil {
		return nil, errdefs.ErrNotFound
	}

	return &task.StateResponse{
		ID:     req.ID,
		Bundle: i.bundle,
		Stdin:  i.stdin,
		Stdout: i.stdout,
		Stderr: i.stderr,
		Pid:    i.pid,
		Status: i.getStatus(),
	}, nil
}
