package wasmtimeshim

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"

	"github.com/bytecodealliance/wasmtime-go"
	taskapi "github.com/containerd/containerd/api/types/task"
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
	shutdownService shutdown.Service

	engine *wasmtime.Engine
	linker *wasmtime.Linker
	store  *wasmtime.Store

	instances *instanceStore
}

func NewService(publisher shim.Publisher, shutdownService shutdown.Service) (task.TaskService, error) {
	engine := wasmtime.NewEngine()

	linker := wasmtime.NewLinker(engine)
	store := wasmtime.NewStore(engine)

	if err := linker.DefineWasi(); err != nil {
		return nil, err
	}

	return &Service{
		publisher:       publisher,
		shutdownService: shutdownService,
		engine:          engine,
		linker:          linker,
		store:           store,
		instances:       &instanceStore{ls: make(map[string]*instanceWrapper)},
	}, nil
}

func (s *Service) Connect(ctx context.Context, req *task.ConnectRequest) (_ *task.ConnectResponse, retErr error) {
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
	if s.instances.Len() > 0 {
		return empty, nil
	}
	s.shutdownService.Shutdown()
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

type instanceWrapper struct {
	i      *wasmtime.Instance
	done   <-chan struct{}
	cancel func()

	bundle string
	stdin  string
	stdout string
	stderr string
	pid    uint32

	mu       sync.Mutex
	cond     *sync.Cond
	status   taskapi.Status
	exitedAt time.Time
	exitCode uint32
}

func (i *instanceWrapper) getStatus() taskapi.Status {
	select {
	case <-i.done:
		return taskapi.StatusStopped
	default:
		i.mu.Lock()
		defer i.mu.Unlock()

		select {
		case <-i.done:
			return taskapi.StatusStopped
		default:
			return i.status
		}
	}
}

type instanceStore struct {
	mu sync.Mutex
	ls map[string]*instanceWrapper
}

func (s *instanceStore) Get(id string) *instanceWrapper {
	s.mu.Lock()
	defer s.mu.Unlock()
	i := s.ls[id]
	return i
}

func (s *instanceStore) Delete(id string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.ls, id)
}

func (s *instanceStore) Add(id string, i *instanceWrapper) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.ls[id] = i
}

func (s *instanceStore) Len() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.ls)
}
