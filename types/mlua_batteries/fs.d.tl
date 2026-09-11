-- mlua-batteries `fs`: file I/O under the host's path policy. Every
-- path is checked before use; a denied or missing path raises. Under
-- `async_overrides` the same functions run on a blocking pool.
local record fs
   -- Raises when the file is larger than the host's max_read_bytes.
   read: function(string): string
   write: function(string, string): boolean
   -- Raw bytes in a Lua string (not required to be UTF-8).
   read_binary: function(string): string
   write_binary: function(string, string): boolean
   copy: function(string, string): boolean
   rename: function(string, string): boolean
   -- symlink(target, linkpath). Unix only.
   symlink: function(string, string): boolean
   exists: function(string): boolean
   is_dir: function(string): boolean
   is_file: function(string): boolean
   -- Creates parents as needed.
   mkdir: function(string): boolean
   -- File or directory (recursive).
   remove: function(string): boolean
   -- Files below the directory, recursively, as paths; bounded by the
   -- host's max_walk_depth / max_walk_entries.
   walk: function(string): {string}
   -- Files matching a glob such as "src/**/*.rs".
   glob: function(string): {string}
end

return fs
