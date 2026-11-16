-- In-process fake LSP server for tests: hover + signature help.
local function server(dispatchers)
  local closing = false
  local srv = {}
  local id = 0
  function srv.request(method, params, callback)
    id = id + 1
    if method == 'initialize' then
      callback(nil, {
        capabilities = {
          hoverProvider = true,
          signatureHelpProvider = { triggerCharacters = { '(', ',' } },
        },
      })
    elseif method == 'textDocument/hover' then
      callback(nil, { contents = { kind = 'markdown', value = '```rust\nfn foo(a: i32, b: u8)\n```\nfake docs' } })
    elseif method == 'textDocument/signatureHelp' then
      local line = vim.api.nvim_get_current_line()
      local _, commas = line:gsub(',', '')
      callback(nil, {
        signatures = {
          {
            label = 'fn foo(a: i32, b: u8)',
            parameters = { { label = 'a: i32', documentation = 'first arg' }, { label = { 15, 20 } } },
          },
        },
        activeSignature = 0,
        activeParameter = commas,
      })
    else
      callback(nil, nil)
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

vim.lsp.start({ name = 'fake', cmd = server, root_dir = vim.fn.getcwd() })
