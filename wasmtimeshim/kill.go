package wasmtimeshim

import (
	"context"
	"fmt"
	"syscall"

	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/runtime/v2/task"
	ptypes "github.com/gogo/protobuf/types"
)

func (s *Service) Kill(ctx context.Context, req *task.KillRequest) (_ *ptypes.Empty, retErr error) {
	defer func() { retErr = wrapErr(retErr, "kill") }()

	if req.ExecID != "" {
		return nil, fmt.Errorf("exec: %w", errdefs.ErrNotImplemented)
	}

	if req.Signal != uint32(syscall.SIGKILL) {
		return nil, fmt.Errorf("signal: %w", errdefs.ErrNotImplemented)
	}

	instance := s.instances.Get(req.ID)
	if instance == nil {
		return nil, errdefs.ErrNotFound
	}

	h, err := s.store.InterruptHandle()
	if err != nil {
		return nil, fmt.Errorf("error getting interupt handle: %w", err)
	}
	h.Interrupt()

	return empty, nil
}
