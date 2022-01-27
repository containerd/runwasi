package wasmtimeshim

import (
	"context"
	"fmt"
	"os"
	"path/filepath"

	taskapi "github.com/containerd/containerd/api/types/task"
	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/mount"
	"github.com/containerd/containerd/runtime/v2/task"
	"golang.org/x/sys/unix"
)

func (s *Service) Delete(ctx context.Context, req *task.DeleteRequest) (_ *task.DeleteResponse, retErr error) {
	defer func() {
		if retErr != nil {
			retErr = fmt.Errorf("delete: %w", retErr)
		}
	}()

	if req.ExecID != "" {
		return nil, fmt.Errorf("exec: %w", errdefs.ErrNotImplemented)
	}

	i := s.instances.Get(req.ID)
	if i == nil {
		return nil, errdefs.ErrNotFound
	}

	switch i.getStatus() {
	case taskapi.StatusRunning, taskapi.StatusPaused, taskapi.StatusPausing:
		return nil, fmt.Errorf("%w: cannot delete running process", errdefs.ErrFailedPrecondition)
	}

	if err := mount.UnmountAll(filepath.Join(i.bundle, "rootfs"), unix.MNT_DETACH); err != nil {
		return nil, fmt.Errorf("unmount bundle: %w", err)
	}

	if err := os.RemoveAll(i.bundle); err != nil {
		return nil, err
	}

	s.instances.Delete(req.ID)

	i.mu.Lock()
	defer i.mu.Unlock()

	return &task.DeleteResponse{
		Pid:        i.pid,
		ExitStatus: i.exitCode,
		ExitedAt:   i.exitedAt,
	}, nil
}
