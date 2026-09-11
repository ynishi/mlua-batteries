-- mlua-batteries `regex`: Rust `regex` crate syntax. An invalid pattern
-- raises.
local record regex
   record Match
      -- 1-based, inclusive.
      start: integer
      stop: integer
      text: string
   end

   is_match: function(string, string): boolean
   -- nil when there is no match.
   find: function(string, string): Match
   find_all: function(string, string): {Match}
   -- [1] is the whole match, [2..] the groups; nil when there is no match.
   captures: function(string, string): {string}
   -- First occurrence only. `$1` / `$name` expand in the replacement.
   replace: function(string, string, string): string
   replace_all: function(string, string, string): string
   split: function(string, string): {string}
end

return regex
