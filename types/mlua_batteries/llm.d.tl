-- mlua-batteries `llm`: chat completion across providers (openai,
-- anthropic, ollama, plus any the host registered). Both the HTTP and
-- the LLM policy must allow a request; `chat` raises on a denied or
-- failed request, `batch` reports per-entry errors instead.
local record llm
   -- type selects which other field is read: text / url / data + media_type.
   record ContentPart   ---@struct
      -- "text" | "image_url" | "image_base64"
      type: string
      text: string         ---@optional
      url: string          ---@optional
      data: string         ---@optional
      media_type: string   ---@optional
   end

   record Message   ---@struct
      -- "user" | "assistant" (system prompts go in Request.system)
      role: string
      content: string | {ContentPart}
   end

   -- Built by the caller: provider and model are required, plus one of
   -- prompt / messages.
   record Request   ---@struct
      provider: string
      model: string
      prompt: string             ---@optional
      messages: {Message}        ---@optional
      system: string             ---@optional
      max_tokens: integer        ---@optional
      temperature: number        ---@optional
      stop: {string}             ---@optional
      api_key: string            ---@optional
      base_url: string           ---@optional
      -- Seconds.
      timeout: integer           ---@optional
      -- Provider-specific fields merged into the request body.
      extra: {string: any}       ---@optional
   end

   record Usage
      input_tokens: integer
      output_tokens: integer
   end

   record Response
      content: string
      -- "stop" | "max_tokens" | "content_filter" | "error"
      finish_reason: string
      usage: Usage
      model: string
      -- Only in `batch` results: set when that request failed, and the
      -- other fields are absent.
      error: string
   end

   chat: function(Request): Response
   -- Runs the requests in parallel; results are in request order.
   batch: function({Request}): {Response}
end

return llm
