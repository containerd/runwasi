package wasmtimeshim

import (
	"context"
	"fmt"
	"os"

	taskapi "github.com/containerd/containerd/api/types/task"
	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/runtime/v2/task"
)

func (s *Service) State(ctx context.Context, req *task.StateRequest) (*task.StateResponse, error) {
	if req.ExecID != "" {
		return nil, fmt.Errorf("exec: %w", errdefs.ErrNotImplemented)
	}

	i := s.instances.Get(req.ID)
	if i == nil {
		s.mu.Lock()
		if req.ID == s.sandboxID {
			s.mu.Unlock()
			// TODO: save sandbox bundle/stdio paths
			cwd, _ := os.Getwd()
			return &task.StateResponse{
				Bundle: cwd,
				ID:     req.ID,
				Pid:    uint32(os.Getpid()),
				Status: taskapi.StatusRunning,
			}, nil
		}
		s.mu.Unlock()
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
