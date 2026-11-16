-- In-process fake language servers for the picker's tests. `fake_lsp(name,
-- opts)` starts one for the current buffer; its answers are whatever
-- `FAKE[name]` holds for the method (a value, or a function of the params, or
-- `{ delay = ms, value = ... }`), so a test sets them after starting.
-- `opts.server_info` is the `serverInfo` it announces; `opts.encoding` is the position encoding it announces; `opts.caps` replaces
-- the capabilities.
_G.FAKE = _G.FAKE or {}
_G.FAKE_CALLS = _G.FAKE_CALLS or {}

local default_caps = {
  definitionProvider = true,
  declarationProvider = true,
  referencesProvider = true,
  implementationProvider = true,
  typeDefinitionProvider = true,
  documentSymbolProvider = true,
  workspaceSymbolProvider = true,
}

function _G.fake_lsp(name, opts)
  opts = opts or {}
  FAKE[name] = FAKE[name] or {}
  local function server(dispatchers)
    -- (For a test to make the server say something: `FAKE_DISPATCH.name.notification(method, params)`.)
    _G.FAKE_DISPATCH = _G.FAKE_DISPATCH or {}
    FAKE_DISPATCH[name] = dispatchers
    local closing, id = false, 0
    local srv = {}
    function srv.request(method, params, callback)
      id = id + 1
      if method == 'initialize' then
        vim.schedule(function()
          local caps = vim.deepcopy(opts.caps or default_caps)
          caps.positionEncoding = opts.encoding or 'utf-16'
          callback(nil, { capabilities = caps, serverInfo = opts.server_info })
        end)
      else
        table.insert(FAKE_CALLS, { name = name, method = method, params = params })
        local answer = FAKE[name][method]
        local delay = 0
        if type(answer) == 'table' and answer.delay then
          delay, answer = answer.delay, answer.value
        end
        if type(answer) == 'function' then
          answer = answer(params)
          -- (A function may delay its answer too.)
          if type(answer) == 'table' and answer.delay then
            delay, answer = answer.delay, answer.value
          end
        end
        vim.defer_fn(function()
          if type(answer) == 'table' and answer.error then
            callback({ code = -32000, message = answer.error }, nil)
          else
            callback(nil, answer)
          end
        end, delay)
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
  return vim.lsp.start({ name = name, cmd = server, root_dir = vim.fn.getcwd() })
end

-- A Location of `path` (a file name), line `l` (from 0), characters c1..c2.
function _G.fake_loc(path, l, c1, c2)
  return {
    uri = vim.uri_from_fname(path),
    range = { start = { line = l, character = c1 }, ['end'] = { line = l, character = c2 } },
  }
end

-- Count what the picker tells the GUI to open, and keep what it says as notifications.
function _G.fake_watch()
  _G.opens, _G.notes = 0, {}
  local notify = vim.rpcnotify
  vim.rpcnotify = function(chan, name, ...)
    if name == 'gui_picker_open' then
      _G.opens = _G.opens + 1
    end
    return notify(chan, name, ...)
  end
  vim.notify = function(msg, level)
    table.insert(_G.notes, msg .. '|' .. tostring(level))
  end
end
