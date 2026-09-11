-- mlua-batteries `string`: Unicode-aware string helpers beyond Lua's
-- built-ins. Every argument must be valid UTF-8 (rejected otherwise).
local record string
   trim: function(string): string
   trim_start: function(string): string
   trim_end: function(string): string
   -- Raises on an empty separator.
   split: function(string, string): {string}
   starts_with: function(string, string): boolean
   ends_with: function(string, string): boolean
   contains: function(string, string): boolean
   -- First occurrence only.
   replace: function(string, string, string): string
   replace_all: function(string, string, string): string
   -- width in characters; fill defaults to a space.
   pad_start: function(string, integer, string): string
   pad_end: function(string, integer, string): string
   -- Cuts to max_len characters, suffix (default "...") included.
   truncate: function(string, integer, string): string
   upper: function(string): string
   lower: function(string): string
   -- One entry per Unicode scalar value.
   chars: function(string): {string}
   char_count: function(string): integer
   reverse: function(string): string
end

return string
