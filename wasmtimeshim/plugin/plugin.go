// package plugin registers the shim as a containerd plugin to use with shim.RunManager.

package plugin

import (
	"github.com/containerd/containerd/pkg/shutdown"
	"github.com/containerd/containerd/plugin"
	"github.com/containerd/containerd/runtime/v2/shim"
	"github.com/cpuguy83/runwasi/wasmtimeshim"
)

func init() {
	plugin.Register(&plugin.Registration{
		Type: plugin.TTRPCPlugin,
		ID:   "task",
		Requires: []plugin.Type{
			plugin.EventPlugin,
			plugin.InternalPlugin,
		},
		InitFn: func(ic *plugin.InitContext) (interface{}, error) {
			pp, err := ic.GetByID(plugin.EventPlugin, "publisher")
			if err != nil {
				return nil, err
			}
			ss, err := ic.GetByID(plugin.InternalPlugin, "shutdown")
			if err != nil {
				return nil, err
			}
			svc, err := wasmtimeshim.NewService(pp.(shim.Publisher), ss.(shutdown.Service))
			if err != nil {
				return nil, err
			}
			return svc, nil
		},
	})
}
