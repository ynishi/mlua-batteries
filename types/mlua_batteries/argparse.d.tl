-- mlua-batteries `argparse`: command-line parsing driven by a spec table.
--
-- Accepts --name value / --name=value / -n value / -nvalue, boolean
-- --flag / --no-flag / bundled -vq, and `--` to end option parsing.
-- `--some-name` and `--some_name` both address the flag `some_name`.
-- Raises (prefix "argparse:") on an unknown option, a wrong-typed value,
-- a missing required flag or positional, a surplus positional, or a
-- malformed spec.
local record argparse
   record Flag
      -- "string" (default) | "integer" | "number" | "boolean"
      type: string
      -- One letter or digit.
      short: string
      -- Used when the flag is absent. Exclusive with required.
      default: any
      required: boolean
      -- Repeatable; the value is a list ({} when absent). Not for boolean.
      multiple: boolean
      help: string
      -- Placeholder shown in usage, e.g. "FILE".
      metavar: string
   end

   -- name is required; everything else has a default.
   record Positional   ---@struct
      name: string
      -- "string" (default) | "integer" | "number"
      type: string         ---@optional
      required: boolean    ---@optional
      -- Collects every remaining argument as a list. Last only.
      rest: boolean        ---@optional
      help: string         ---@optional
   end

   record Spec
      -- Program name shown in usage.
      name: string
      description: string
      flags: {string: Flag}
      positionals: {Positional}
      -- Unknown options and surplus positionals go to Result.rest
      -- instead of raising.
      allow_unknown: boolean
   end

   record Result
      -- Every flag by name: the value, its default, {} for an absent
      -- multiple flag, nil otherwise.
      opts: {string: any}
      -- Declared positionals by name; a rest positional is a {string}.
      args: {string: any}
      -- Only non-empty under allow_unknown.
      rest: {string}
   end

   parse: function({string}, Spec): Result
   usage: function(Spec): string
end

return argparse
