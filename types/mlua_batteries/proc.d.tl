-- mlua-batteries `proc`: spawn a pipeline of processes from pre-split
-- argv arrays, no shell involved. Raises on a malformed spec or a spawn
-- failure; a non-zero exit is reported in the result, not raised.
local record proc
   record Stage   ---@struct
      argv: {string}
      env: {string: string}   ---@optional
   end

   record FileRef   ---@struct
      path: string
      -- stdout_to only.
      append: boolean   ---@optional
   end

   record Options
      cwd: string
      stdin_from: FileRef
      stdout_to: FileRef
      timeout_secs: number
   end

   record StageResult
      -- nil when killed by a signal or the timeout.
      exit_code: integer
      -- Only the last stage's stdout is captured; tail-capped.
      stdout: string
      stderr: string
      duration_ms: integer
   end

   record Result
      -- Every stage exited 0 and nothing timed out.
      ok: boolean
      timed_out: boolean
      duration_ms: integer
      stages: {StageResult}
   end

   pipeline: function(stages: {Stage}, opts?: Options): Result
end

return proc
