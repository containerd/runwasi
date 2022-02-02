package wasmtimeshim

import (
	"context"
	"fmt"
	"syscall"

	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/runtime/v2/task"
	ptypes "github.com/gogo/protobuf/types"
	"golang.org/x/sys/unix"
)

func (s *Service) Kill(ctx context.Context, req *task.KillRequest) (_ *ptypes.Empty, retErr error) {
	defer func() {
		if retErr != nil {
			fmt.Errorf("kill: %w", retErr)
		}
	}()

	if req.ExecID != "" {
		return nil, fmt.Errorf("exec: %w", errdefs.ErrNotImplemented)
	}

	client, err := s.getSandboxClient(ctx)
	if err != nil {
		return nil, err
	}

	if req.ID == s.sandboxID && syscall.Signal(req.Signal) == syscall.SIGKILL {
		s.cmd.Process.Kill()
		s.cmd.Wait()
		return &ptypes.Empty{}, nil
	}

	return client.Kill(ctx, req)
}

func (s *Sandbox) Kill(ctx context.Context, req *task.KillRequest) (*ptypes.Empty, error) {
	if syscall.Signal(req.Signal) == unix.SIGKILL {
		h, err := s.store.InterruptHandle()
		if err != nil {
			return nil, fmt.Errorf("error getting interrupt handle: %w", err)
		}

		h.Interrupt()
		return &ptypes.Empty{}, nil
	}

	// TODO: Maybe some configurable function could be called in the wasm module?

	return nil, fmt.Errorf("signal: %w", errdefs.ErrNotImplemented)
}
