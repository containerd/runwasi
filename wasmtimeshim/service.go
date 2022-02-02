package wasmtimeshim

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"time"

	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/pkg/shutdown"
	"github.com/containerd/containerd/runtime/v2/shim"
	"github.com/containerd/containerd/runtime/v2/task"
	"github.com/containerd/ttrpc"
	ptypes "github.com/gogo/protobuf/types"
	"github.com/sirupsen/logrus"
)

var empty = &ptypes.Empty{}

type Service struct {
	publisher       shim.Publisher
	sandboxBin      string
	shutdownService shutdown.Service

	mu            sync.Mutex
	cond          *sync.Cond
	sandboxClient task.TaskService
	sandboxID     string
	pid           uint32
	cmd           *exec.Cmd
	exitedAt      time.Time
}

func NewService(publisher shim.Publisher, shutdownService shutdown.Service, sandboxBin string) (task.TaskService, error) {
	s := &Service{
		publisher:       publisher,
		sandboxBin:      sandboxBin,
		shutdownService: shutdownService,
	}
	s.cond = sync.NewCond(&s.mu)
	return s, nil
}

func (s *Service) Connect(ctx context.Context, req *task.ConnectRequest) (_ *task.ConnectResponse, retErr error) {
	client, err := s.getSandboxClient(ctx)
	if err != nil {
		return nil, err
	}
	return client.Connect(ctx, req)
}

func (s *Sandbox) Connect(ctx context.Context, req *task.ConnectRequest) (_ *task.ConnectResponse, retErr error) {
	defer func() {
		if retErr != nil {
			retErr = fmt.Errorf("connect: %w", retErr)
		}
	}()

	i := s.instances.Get(req.ID)
	if i == nil {
		return nil, errdefs.ErrNotFound
	}

	return &task.ConnectResponse{
		ShimPid: uint32(os.Getpid()),
		TaskPid: i.pid,
	}, nil
}

func (s *Service) Shutdown(ctx context.Context, req *task.ShutdownRequest) (*ptypes.Empty, error) {
	return empty, nil
}

func (s *Sandbox) Shutdown(ctx context.Context, req *task.ShutdownRequest) (*ptypes.Empty, error) {
	return empty, nil
}

func (s *Service) RegisterTTRPC(server *ttrpc.Server) error {
	logrus.Debugf("Registering wasm task service")
	task.RegisterTaskService(server, s)
	return nil
}

func readBundleConfig(bundle, name string, i interface{}) error {
	data, err := os.ReadFile(filepath.Join(bundle, name+".json"))
	if err != nil {
		return fmt.Errorf("read config: %w", err)
	}

	if err := json.Unmarshal(data, i); err != nil {
		return fmt.Errorf("unmarshal config: %w", err)
	}
	return nil
}
