package wasmtimeshim

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync"

	"github.com/bytecodealliance/wasmtime-go"
	taskapi "github.com/containerd/containerd/api/types/task"
	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/mount"
	"github.com/containerd/containerd/pkg/cri/annotations"
	"github.com/containerd/containerd/runtime/v2/task"
	"github.com/cpuguy83/runwasi/wasmtimeoci"
	"github.com/opencontainers/runtime-spec/specs-go"
	"golang.org/x/sys/unix"
)

func (s *Service) Create(ctx context.Context, req *task.CreateTaskRequest) (_ *task.CreateTaskResponse, retErr error) {
	defer func() {
		if retErr != nil {
			retErr = fmt.Errorf("create: %w", retErr)
		}
	}()

	if req.Checkpoint != "" || req.ParentCheckpoint != "" {
		return nil, fmt.Errorf("checkpoint: %w", errdefs.ErrNotImplemented)
	}

	if req.Terminal {
		return nil, fmt.Errorf("terminal: %w", errdefs.ErrNotImplemented)
	}

	var spec specs.Spec
	if err := readBundleConfig(req.Bundle, "config", &spec); err != nil {
		return nil, err
	}

	if _, ok := spec.Annotations[annotations.SandboxID]; ok {
		s.mu.Lock()
		if !s.sandboxCreated {
			// This is the first container in the cri sandbox ("pause"), which we can't really run, nor is there any real point to.
			s.sandboxCreated = true
			s.sandboxID = req.ID
			s.mu.Unlock()
			return &task.CreateTaskResponse{
				Pid: uint32(os.Getpid()),
			}, nil
		}
		s.mu.Unlock()
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

	return &task.CreateTaskResponse{Pid: pid}, nil
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
