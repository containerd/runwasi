package wasmtimeshim

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"time"

	"github.com/bytecodealliance/wasmtime-go"
	taskapi "github.com/containerd/containerd/api/types/task"
	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/mount"
	"github.com/containerd/containerd/pkg/cri/annotations"
	"github.com/containerd/containerd/runtime/v2/task"
	"github.com/containerd/ttrpc"
	"github.com/cpuguy83/runwasi/wasmtimeoci"
	"github.com/opencontainers/runtime-spec/specs-go"
	"golang.org/x/sys/unix"
)

func (s *Service) Create(ctx context.Context, req *task.CreateTaskRequest) (_ *task.CreateTaskResponse, retErr error) {
	defer func() {
		if retErr != nil {
			os.RemoveAll(req.Bundle)
			retErr = fmt.Errorf("create: %w", retErr)
		}
	}()

	if req.Checkpoint != "" || req.ParentCheckpoint != "" {
		return nil, fmt.Errorf("checkpoint: %w", errdefs.ErrNotImplemented)
	}

	if req.Terminal {
		return nil, fmt.Errorf("terminal: %w", errdefs.ErrNotImplemented)
	}

	s.mu.Lock()
	if client := s.sandboxClient; client != nil {
		s.mu.Unlock()
		return client.Create(ctx, req)
	}
	s.mu.Unlock()

	var spec specs.Spec
	if err := readBundleConfig(req.Bundle, "config", &spec); err != nil {
		return nil, err
	}

	cmd := exec.Command(s.sandboxBin)
	cmd.Env = append(cmd.Env, "_RUNWASI_SANDBOX=1")

	for _, ns := range spec.Linux.Namespaces {
		if ns.Type == specs.NetworkNamespace {
			cmd.Env = append(cmd.Env, fmt.Sprintf("%s=%s", "_RUNWASI_NETNS_PATH", ns.Path))
			break
		}
	}

	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("error starting sandbox: %w", err)
	}
	go func() {
		cmd.Wait()
		now := time.Now()
		s.mu.Lock()
		s.exitedAt = now
		s.mu.Unlock()
	}()

	dialer := &net.Dialer{}
	var (
		conn net.Conn
		err  error
	)

	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	for {
		conn, err = dialer.DialContext(ctx, "unix", "sandbox.sock")
		if err != nil {
			if errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
				// TODO: logs from sandbox
				return nil, err
			}
			continue
		}
		break
	}

	sandbox := task.NewTaskClient(ttrpc.NewClient(conn))
	s.mu.Lock()
	s.sandboxClient = sandbox
	s.cond.Broadcast()
	s.pid = uint32(cmd.Process.Pid)
	s.sandboxID = req.ID
	s.cmd = cmd
	s.mu.Unlock()

	// If there is no sandbox grouping from cri, then we need to create the task for real here.
	if _, ok := spec.Annotations[annotations.SandboxID]; !ok {
		return sandbox.Create(ctx, req)
	}

	return &task.CreateTaskResponse{Pid: uint32(cmd.Process.Pid)}, nil
}

func configureStdio(setStdioFile func(string) error, p string) (*os.File, error) {
	if p == "" {
		return nil, nil
	}

	f, err := os.OpenFile(p, os.O_RDWR, 0)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	defer f.Close()

	if err := setStdioFile(p); err != nil {
		return nil, err
	}

	return f, nil
}

func (s *Sandbox) Create(ctx context.Context, unmarshal func(i interface{}) error) (_ interface{}, retErr error) {
	req := task.CreateTaskRequest{}
	if err := unmarshal(&req); err != nil {
		return nil, err
	}

	var spec specs.Spec
	if err := readBundleConfig(req.Bundle, "config", &spec); err != nil {
		return nil, err
	}

	if len(req.Rootfs) > 0 {
		mounts := make([]mount.Mount, 0, len(req.Rootfs))
		for _, m := range req.Rootfs {
			mounts = append(mounts, mount.Mount{
				Type:    m.Type,
				Source:  m.Source,
				Options: m.Options,
			})
		}
		if err := mount.All(mounts, filepath.Join(req.Bundle, "rootfs")); err != nil {
			return nil, fmt.Errorf("mount rootfs: %w", err)
		}
		defer func() {
			if retErr != nil {
				mount.UnmountAll(filepath.Join(req.Bundle, "rootfs"), unix.MNT_DETACH)
			}
		}()
	}

	wasi := wasmtime.NewWasiConfig()

	if err := wasmtimeoci.PrepareEnv(spec.Process, wasi); err != nil {
		return nil, err
	}
	rootfs, err := wasmtimeoci.PrepareRootfs(&spec, req.Bundle, wasi)
	if err != nil {
		return nil, err
	}

	p := filepath.Join(rootfs, spec.Process.Args[0])
	if _, err := os.Stat(p); err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return nil, fmt.Errorf("%s: %w", p, errdefs.ErrNotFound)
		}
		return nil, err
	}
	mod, err := wasmtime.NewModuleFromFile(s.engine, p)
	if err != nil {
		return nil, err
	}

	wasi.SetArgv(spec.Process.Args)

	cleanup := func(closer io.Closer) {
		if retErr != nil {
			closer.Close()
		}
	}

	stdin, err := configureStdio(wasi.SetStdinFile, req.Stdin)
	if err != nil {
		return nil, fmt.Errorf("stdin: %w", err)
	}
	defer cleanup(stdin)

	stdout, err := configureStdio(wasi.SetStdoutFile, req.Stdout)
	if err != nil {
		return nil, fmt.Errorf("stdout: %w", err)
	}
	defer cleanup(stdout)

	stderr, err := configureStdio(wasi.SetStderrFile, req.Stderr)
	if err != nil {
		return nil, fmt.Errorf("stderr: %w", err)
	}
	defer cleanup(stderr)

	s.store.SetWasi(wasi)

	instance, err := s.linker.Instantiate(s.store, mod)
	if err != nil {
		return nil, err
	}

	pid := uint32(os.Getpid())

	iCtx, cancel := context.WithCancel(context.Background())
	i := &instanceWrapper{
		i:      instance,
		done:   iCtx.Done(),
		cancel: cancel,
		bundle: req.Bundle,
		stdin:  req.Stdin,
		stdout: req.Stdout,
		stderr: req.Stderr,
		pid:    pid,
		status: taskapi.StatusCreated,
	}
	i.cond = sync.NewCond(&i.mu)

	s.instances.Add(req.ID, i)

	return &task.CreateTaskResponse{Pid: uint32(os.Getpid())}, nil
}
