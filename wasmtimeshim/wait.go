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

	if req.ID == s.sandboxID {
		return s.wait(ctx)
	}

	client, err := s.getSandboxClient(ctx)
	if err != nil {
		return nil, err
	}
	return client.Wait(ctx, req)
}

func (s *Service) wait(ctx context.Context) (*task.WaitResponse, error) {
	s.cmd.Wait()

	s.mu.Lock()
	defer s.mu.Unlock()

	var t time.Time

	for s.exitedAt.Equal(t) {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		s.cond.Wait()
	}

	return &task.WaitResponse{
		ExitStatus: uint32(s.cmd.ProcessState.ExitCode()),
		ExitedAt:   s.exitedAt,
	}, nil
}

func (s *Sandbox) Wait(ctx context.Context, req *task.WaitRequest) (_ *task.WaitResponse, retErr error) {
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
