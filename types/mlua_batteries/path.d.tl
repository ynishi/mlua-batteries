-- mlua-batteries `path`: path manipulation. Pure string computation
-- except `absolute`, which resolves through the filesystem (and the
-- host's path policy) and raises when the path does not exist.
local record path
   join: function(...: string): string
   -- nil when there is no parent / file name / stem / extension.
   parent: function(string): string
   filename: function(string): string
   stem: function(string): string
   ext: function(string): string
   absolute: function(string): string
   is_absolute: function(string): boolean
end

return path
