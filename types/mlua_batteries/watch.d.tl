-- mlua-batteries `watch`: filesystem versioning watcher. A directory
-- tree is watched on a Rust thread; each content change lands in a
-- content-addressed store plus a JSONL journal under `store_dir`.
-- Not routed through the host's path policy.
local record watch
   record Options
      -- Defaults to ~/.local/share/guard-mcp.
      store_dir: string
   end

   -- The watch runs until stop() or garbage collection: keep a reference.
   record Handle
      stop: function(Handle): boolean
      running: function(Handle): boolean
   end

   record Version
      -- Epoch seconds; nil for malformed journal lines.
      ts: number
      -- "create" | "modify" | "delete" | "skip_too_large" | "gap"
      event: string
      -- nil for delete / gap.
      sha: string
      size: integer
      path: string
   end

   record Restored
      restored: string
      sha: string
      size: integer
   end

   start: function(string, Options): Handle
   -- Exact journaled path or a path suffix, oldest first.
   history: function(string, Options): {Version}
   -- restore(dest_path, sha, opts?)
   restore: function(string, string, Options): Restored
end

return watch
