package wasmtimeshim

import (
	"context"
	"fmt"

	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/runtime/v2/task"
	ptypes "github.com/gogo/protobuf/types"
)

func (s *Service) Pids(ctx context.Context, req *task.PidsRequest) (*task.PidsResponse, error) {
	return nil, fmt.Errorf("pids: %w", errdefs.ErrNotImplemented)
}

func (s *Service) Pause(ctx context.Context, req *task.PauseRequest) (*ptypes.Empty, error) {
	return nil, fmt.Errorf("pause: %w", errdefs.ErrNotImplemented)
}
func (s *Service) Resume(ctx context.Context, req *task.ResumeRequest) (*ptypes.Empty, error) {
	return nil, fmt.Errorf("resume: %w", errdefs.ErrNotImplemented)
}

func (s *Service) Checkpoint(ctx context.Context, req *task.CheckpointTaskRequest) (*ptypes.Empty, error) {
	return nil, fmt.Errorf("checkpoint: %w", errdefs.ErrNotImplemented)
}

func (s *Service) Kill(ctx context.Context, req *task.KillRequest) (*ptypes.Empty, error) {
	return nil, fmt.Errorf("kill: %w", errdefs.ErrNotImplemented)
}

func (s *Service) Exec(ctx context.Context, req *task.ExecProcessRequest) (*ptypes.Empty, error) {
	return nil, fmt.Errorf("exec: %w", errdefs.ErrNotImplemented)
}

func (s *Service) ResizePty(ctx context.Context, req *task.ResizePtyRequest) (*ptypes.Empty, error) {
	return nil, fmt.Errorf("resizepty: %w", errdefs.ErrNotImplemented)
}

func (s *Service) CloseIO(ctx context.Context, req *task.CloseIORequest) (*ptypes.Empty, error) {
	return nil, fmt.Errorf("closeio: %w", errdefs.ErrNotImplemented)
}

func (s *Service) Update(ctx context.Context, req *task.UpdateTaskRequest) (*ptypes.Empty, error) {
	return nil, fmt.Errorf("update: %w", errdefs.ErrNotImplemented)
}

func (s *Service) Stats(ctx context.Context, req *task.StatsRequest) (*task.StatsResponse, error) {
	return nil, fmt.Errorf("stats: %w", errdefs.ErrNotImplemented)
}
