-- In-process fake language server for completion tests: items with
-- documentation (resolved later, as lua_ls does), and a multi-line snippet.
local items = {
  { label = 'byte', kind = 3, detail = 'function string.byte(s)' },
  { label = 'for', kind = 14 },
  { label = 'format', kind = 3, detail = 'function string.format(fmt)', documentation = 'early docs of format' },
  {
    label = 'function',
    kind = 15,
    insertTextFormat = 2,
    insertText = 'function ($1)\n\t$0\nend',
    detail = 'function () end',
  },
  { label = 'rep', kind = 3 },
}

local function server(dispatchers)
  local closing, id = false, 0
  local srv = {}
  function srv.request(method, params, callback)
    id = id + 1
    -- (Answers come later, as a server's do: some calls can't run inside
    -- the request.)
    if method == 'initialize' then
      vim.schedule(function()
        callback(nil, {
          capabilities = { completionProvider = { resolveProvider = true, triggerCharacters = { '.' } } },
        })
      end)
    elseif method == 'textDocument/completion' then
      vim.schedule(function()
        callback(nil, { isIncomplete = false, items = vim.deepcopy(items) })
      end)
    elseif method == 'completionItem/resolve' then
      local item = vim.deepcopy(params)
      item.documentation = { kind = 'markdown', value = 'RESOLVED docs of ' .. item.label }
      vim.defer_fn(function()
        callback(nil, item)
      end, 30)
    else
      vim.schedule(function()
        callback(nil, nil)
      end)
    end
    return true, id
  end
  function srv.notify(method)
    if method == 'exit' then
      dispatchers.on_exit(0, 15)
    end
    return true
  end
  function srv.is_closing()
    return closing
  end
  function srv.terminate()
    closing = true
  end
  return srv
end

vim.lsp.start({ name = 'fakecmp', cmd = server, root_dir = vim.fn.getcwd() })
