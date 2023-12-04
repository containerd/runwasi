(component
  (core module $m
      (func (export "thunk"))
      (func (export "thunk-trap") unreachable)
  )
  (core instance $i (instantiate $m))
  (func (export "thunk")
      (canon lift (core func $i "thunk"))
  )
  (func (export "thunk-trap")
      (canon lift (core func $i "thunk-trap"))
  )
)