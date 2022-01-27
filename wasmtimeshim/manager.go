package wasmtimeshim

import (
	"context"
	"fmt"
	"io"
	"io/ioutil"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"
	"time"

	"github.com/containerd/containerd/errdefs"
	"github.com/containerd/containerd/namespaces"
	"github.com/containerd/containerd/runtime/v2/shim"
	"github.com/containerd/containerd/runtime/v2/task"
	"github.com/containerd/ttrpc"
)

func New(name string) *Manager {
	return &Manager{name}
}

type Manager struct {
	name string
}

func (m *Manager) Name() string {
	return m.name
}

func (m *Manager) Start(ctx context.Context, id string, opts shim.StartOpts) (_ string, retErr error) {
	ns, err := namespaces.NamespaceRequired(ctx)
	if err != nil {
		return "", err
	}

	bin, err := os.Executable()
	if err != nil {
		return "", err
	}

	args := []string{
		"-id", id,
		"-namespace", ns,
		"-address", opts.Address,
	}

	cwd, err := os.Getwd()
	if err != nil {
		return "", err
	}

	cmd := exec.Command(bin, args...)
	cmd.Dir = cwd
	cmd.Env = os.Environ()

	cmd.SysProcAttr = &syscall.SysProcAttr{
		Cloneflags: syscall.CLONE_NEWNS,
	}

	addr, err := shim.SocketAddress(ctx, opts.Address, id)
	if err != nil {
		return "", err
	}

	socket, err := shim.NewSocket(addr)
	if err != nil {
		if !shim.SocketEaddrinuse(err) {
			return "", fmt.Errorf("create new shim socket: %w", err)
		}
		if shim.CanConnect(addr) {
			if err := shim.WriteAddress("address", addr); err != nil {
				return "", fmt.Errorf("write existing socket for shim: %w", err)
			}
			return addr, nil
		}
		if err := shim.RemoveSocket(addr); err != nil {
			return "", fmt.Errorf("remove pre-existing socket: %w", err)
		}
		if socket, err = shim.NewSocket(addr); err != nil {
			return "", fmt.Errorf("try create new shim socket 2x: %w", err)
		}
	}

	defer func() {
		if retErr != nil {
			socket.Close()
			shim.RemoveSocket(addr)
		}
	}()

	if err := shim.WriteAddress("address", addr); err != nil {
		return "", err
	}

	f, err := socket.File()
	if err != nil {
		return "", err
	}
	cmd.ExtraFiles = append(cmd.ExtraFiles, f)

	if err := cmd.Start(); err != nil {
		f.Close()
		return "", err
	}
	go cmd.Wait()

	// Drain whatever config is in stdin for now.
	// TODO: support shim configs like ShimCgroup in the runc shim?
	io.Copy(ioutil.Discard, os.Stdin)

	return addr, nil
}

func (m *Manager) Stop(ctx context.Context, id string) (shim.StopStatus, error) {
	cwd, err := os.Getwd()
	if err != nil {
		return shim.StopStatus{}, nil
	}

	addr, err := shim.ReadAddress(filepath.Join(cwd, "address"))
	if err != nil {
		return shim.StopStatus{}, err
	}

	timeout := 10 * time.Second
	if dl, ok := ctx.Deadline(); ok {
		if t := dl.Sub(time.Now()); t > 0 {
			timeout = t
		}
	}

	conn, err := shim.AnonDialer(addr, timeout)
	if err != nil {
		return shim.StopStatus{}, err
	}
	defer conn.Close()

	client := task.NewTaskClient(ttrpc.NewClient(conn))
	resp, err := client.Delete(ctx, &task.DeleteRequest{ID: id})
	if err != nil && !errdefs.IsNotFound(err) {
		return shim.StopStatus{}, err
	}

	return shim.StopStatus{Pid: int(resp.Pid), ExitStatus: int(resp.ExitStatus), ExitedAt: resp.ExitedAt}, nil
}
