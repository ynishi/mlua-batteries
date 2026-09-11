-- mlua-batteries `env`: environment variables behind the host's policy.
--
-- `set` writes an overlay seen by later `get` calls in this VM only; the
-- process environment is never modified. Reads and writes of a variable
-- the policy denies raise.
local record env
   -- nil when the variable is unset.
   get: function(string): string
   get_or: function(string, string): string
   set: function(string, string)
   -- HOME (or USERPROFILE); nil when neither is set.
   home: function(): string
end

return env
