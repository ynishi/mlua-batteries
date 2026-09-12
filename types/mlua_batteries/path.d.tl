-- mlua-batteries `path`: path manipulation. Pure string computation
-- except `absolute`, which resolves through the filesystem (and the
-- host's path policy) and raises when the path does not exist.
local record path
   join: function(...: string): string
   -- nil when there is no parent / file name / stem / extension.
   parent: function(p: string): string     ---@nilable
   filename: function(p: string): string   ---@nilable
   stem: function(p: string): string       ---@nilable
   ext: function(p: string): string        ---@nilable
   absolute: function(string): string
   is_absolute: function(string): boolean
end

return path
