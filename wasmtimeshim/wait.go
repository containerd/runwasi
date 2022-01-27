package wasmtimeshim

import (
	"context"
	"fmt"

	taskapi "github.com/containerd/containerd/api/types/task"
	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/runtime/v2/task"
)

func (s *Service) Wait(ctx context.Context, req *task.WaitRequest) (_ *task.WaitResponse, retErr error) {
	defer func() {
		if retErr != nil {
			retErr = fmt.Errorf("wait: %w", retErr)
		}
	}()

	if req.ExecID != "" {
		return nil, fmt.Errorf("exec: %w", errdefs.ErrNotImplemented)
	}

	i := s.instances.Get(req.ID)
	if i == nil {
		return nil, errdefs.ErrNotFound
	}

	var resp task.WaitResponse
	i.mu.Lock()
	for i.status != taskapi.StatusStopped {
		select {
		case <-ctx.Done():
			i.mu.Unlock()
			return nil, ctx.Err()
		default:
		}
		i.cond.Wait()
	}
	resp.ExitStatus = i.exitCode
	resp.ExitedAt = i.exitedAt
	i.mu.Unlock()

	return &resp, nil
}
