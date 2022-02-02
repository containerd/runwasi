package wasmtimeshim

import (
	"sync"
	"time"

	"github.com/bytecodealliance/wasmtime-go"
	taskapi "github.com/containerd/containerd/api/types/task"
)

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

func newInstanceStore() *instanceStore {
	return &instanceStore{
		ls: make(map[string]*instanceWrapper),
	}
}
