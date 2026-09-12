-- mlua-batteries `task`: structured async tasks with cooperative
-- cancellation. Needs a tokio current-thread runtime driving a LocalSet
-- on the host side; from Lua every suspension point is an ordinary
-- call. A cancelled task raises "task cancelled" at its next checkpoint.
local record task
   record CancelToken
      cancel: function(CancelToken)
      is_cancelled: function(CancelToken): boolean
      -- Raises when cancelled.
      check: function(CancelToken)
   end

   record Handle
      id: string
      -- nil unless SpawnOptions.name was given.
      name: string
      -- The task's return value; raises with its error. Once only.
      join: function(Handle): any
      abort: function(Handle)
      is_finished: function(Handle): boolean
      -- Milliseconds since spawn.
      elapsed: function(Handle): number
   end

   record Scope
      name: string
      spawn: function(self: Scope, body: (function(): any), opts?: SpawnOptions): Handle
      cancel: function(Scope)
      token: function(Scope): CancelToken
   end

   record SpawnOptions
      name: string
      -- "coroutine" | "async_fn"; the host's TaskConfig default when absent.
      driver: string
   end

   record TimeoutOptions
      -- Drain budget after the deadline, before the hard abort.
      grace_ms: number
   end

   record Current
      id: string
      name: string
      cancelled: boolean
   end

   -- Attaches to the current scope (the VM root when none is active).
   spawn: function(body: (function(): any), opts?: SpawnOptions): Handle
   -- Milliseconds.
   sleep: function(number)
   yield: function()
   checkpoint: function()
   cancel_token: function(): CancelToken
   current: function(): Current
   -- Waits for every task spawned into the scope before returning the
   -- body's result.
   scope: function(body: (function(Scope): any)): any
   scope: function(name: string, body: (function(Scope): any)): any
   -- with_timeout(ms, body, opts?): deadline, then grace, then abort.
   with_timeout: function(ms: number, body: (function(Scope): any), opts?: TimeoutOptions): any
end

return task
