package wasmtimeshim

import (
	"context"
	"fmt"
	"time"

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

	s.mu.Lock()
	if s.sandboxID == req.ID {
		s.mu.Unlock()
		ch := make(chan struct{})
		ctx, cancel := context.WithCancel(ctx)
		defer cancel()

		s.shutdownService.RegisterCallback(func(ctx context.Context) error {
			close(ch)
			return nil
		})

		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-ch:
			return &task.WaitResponse{
				ExitStatus: uint32(0),
				ExitedAt:   time.Now(),
			}, nil
		}
	}
	s.mu.Unlock()

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
