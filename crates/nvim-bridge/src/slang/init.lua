-- Slang (SystemVerilog) support in the app: the Hierarchy and Trace views (`hierarchy.lua`, `trace.lua`), the
-- client side of what slang-server asks of its editor (`slang.quickPick`, `slang.showInHierarchy`,
-- `slang/activeInstanceChanged`), the hierarchy search, `:EshviSlang`, and the setup of svtrace's LSP client
-- from the binaries that ship beside the app (`$ESHVI_BIN`). They replace slang-server.nvim.
-- Modules `eshvi.slang`, `eshvi.slang.hierarchy` and `eshvi.slang.trace`, put in `package.preload` by
-- `slang_load.lua` (`lib.rs`) for the own config; `require('eshvi.slang').setup()` runs once.
-- NOTES.md "SystemVerilog hierarchy". Opt out: `vim.g.eshvi_slang = false`.
--
--   M.setup()                    the autocmds, client commands, `:EshviSlang` and svtrace's client
--   M.search()                   the hierarchy search, in the native picker; the choice is revealed in the Hierarchy view
--   M.select_active()            the active-instance code lens on the cursor's line, as a quick pick
--   M.up()                       from the active instance's module: its instantiation, the instance around it active
local hierarchy = require('eshvi.slang.hierarchy')
local trace = require('eshvi.slang.trace')

local M = {}

local SERVER = hierarchy.SERVER
local SVTRACE = hierarchy.SVTRACE

-- How long a typed query waits before it is sent (slang-server.nvim's `search.query_delay`), in ms.
local QUERY_DELAY = 150

local function error_message(message)
  vim.notify(tostring(message), vim.log.levels.ERROR)
end

-- ---- The build svtrace elaborates ----

-- svtrace elaborates the build slang-server ends up using. slang-server's `slang.setBuildFile { file }` makes
-- that file the design (and forgets a top file); `slang.setTopLevel { file }` makes a top file the design with
-- no build file, which svtrace can't elaborate; either with "" is explore mode, no design. So a build file is
-- `svtrace.useBuild { file }` and everything else `{ file = null }`: svtrace has no design and the trace asks
-- slang-server. Explore mode is `{ file = null, explore = true }`: a change of `server.json` takes slang-server
-- from it back to the configured build, and svtrace follows; a top file stays. (What slang-server does by itself, as loading a build file for a waveform, isn't a request
-- and isn't seen.) The choice is in memory, as the server's: kept here for an svtrace that attaches later.
local desired_build -- nil: nothing chosen; false: no design; string: a build file
local desired_explore = false -- with `false`: explore mode, not a top file

local function use_build(file, explore)
  local client = hierarchy.find_client(SVTRACE)
  local provider = client and client.server_capabilities and client.server_capabilities.executeCommandProvider
  if not (provider and vim.tbl_contains(provider.commands or {}, 'svtrace.useBuild')) then
    return
  end
  hierarchy.execute('svtrace.useBuild', { { file = file or vim.NIL, explore = (not file and explore) or nil } }, function(err)
    if err then
      vim.notify('svtrace: ' .. tostring(err), vim.log.levels.WARN)
      return
    end
    -- The design is another one: the Hierarchy asks again (what is open stays open).
    if hierarchy.registered() then
      hierarchy.refresh({ expanded = {} })
    end
  end, SVTRACE)
end

local function mirror_build(command, file)
  if command == 'slang.setBuildFile' and type(file) == 'string' and file ~= '' then
    desired_build = file
  else
    desired_build = false
  end
  desired_explore = desired_build == false and (type(file) ~= 'string' or file == '')
  use_build(desired_build or nil, desired_explore)
end

-- Wraps the slang-server client's `request`: a `workspace/executeCommand` of `slang.setBuildFile` or
-- `slang.setTopLevel` that succeeds, from wherever it comes (`:EshviSlang`, a code lens, a quick pick), is
-- mirrored to svtrace.
local function watch_builds(client)
  if client._eshvi_slang_builds then
    return
  end
  client._eshvi_slang_builds = true
  local request = client.request
  client.request = function(self, method, params, handler, bufnr)
    local command = method == 'workspace/executeCommand' and type(params) == 'table' and params.command
    if command == 'slang.setBuildFile' or command == 'slang.setTopLevel' then
      local file = params.arguments and params.arguments[1]
      local inner = handler or self.handlers[method] or vim.lsp.handlers[method]
      handler = function(err, ...)
        if not err then
          mirror_build(command, file)
        end
        if inner then
          return inner(err, ...)
        end
      end
    end
    return request(self, method, params, handler, bufnr)
  end
end

-- ---- Views, as a client attaches ----

local function is_slang_client(client)
  local name = client and client.server_info and client.server_info.name
  return name == SERVER or name == SVTRACE
end

local function show_views()
  hierarchy.install()
  hierarchy.register()
  trace.register()
end

local function hide_views()
  hierarchy.unregister()
  trace.unregister()
end

local group

local function watch_clients()
  group = vim.api.nvim_create_augroup('eshvi_slang', { clear = true })
  -- The views come with the first slang-server or svtrace client, so a user of other languages sees no
  -- slang buttons, and go with the last one (a stopped server takes them).
  vim.api.nvim_create_autocmd('LspAttach', {
    group = group,
    callback = function(args)
      local client = args.data and vim.lsp.get_client_by_id(args.data.client_id)
      if is_slang_client(client) then
        show_views()
        if client.server_info.name == SERVER then
          watch_builds(client)
        elseif desired_build ~= nil and not client._eshvi_slang_builds then
          -- An svtrace that starts after the server's build was chosen: told which one.
          client._eshvi_slang_builds = true
          use_build(desired_build or nil, desired_explore)
        end
      end
    end,
  })
  vim.api.nvim_create_autocmd('LspDetach', {
    group = group,
    callback = function()
      -- (Whichever client it was: the buffer is still in its list while this runs, and a client that
      -- stopped is gone from `get_client_by_id`; so look when it has settled.)
      vim.schedule(function()
        if (hierarchy.registered() or trace.registered()) and not hierarchy.any_attached() then
          hide_views()
        end
      end)
    end,
  })
  -- Already attached before this ran.
  vim.schedule(function()
    if hierarchy.any_attached() then
      show_views()
      for _, client in ipairs(vim.lsp.get_clients()) do
        if client.server_info and client.server_info.name == SERVER then
          watch_builds(client)
        end
      end
    end
  end)
end

-- ---- What the server asks of its editor ----

-- `slang.quickPick`: the server's code lenses ("select the active instance"). The items' `label` and
-- `description` in the native picker, then the chosen `value` goes back with `onSelectCommand`.
local function quick_pick(command, ctx)
  local params = command.arguments and command.arguments[1]
  if type(params) ~= 'table' or type(params.items) ~= 'table' or type(params.onSelectCommand) ~= 'string' then
    error_message('slang-server: invalid quick pick command')
    return
  end
  local function choose(item)
    if not item then
      return
    end
    local client = vim.lsp.get_client_by_id(ctx.client_id)
    if not client then
      error_message('slang-server: quick pick LSP client is no longer available')
      return
    end
    local completed = false
    local sent = client:request(
      'workspace/executeCommand',
      { command = params.onSelectCommand, arguments = { item.value } },
      function(err)
        if completed then
          return
        end
        completed = true
        if err then
          error_message(err.message)
        end
      end,
      ctx.bufnr
    )
    if sent == false and not completed then
      completed = true
      error_message('slang-server: failed to send workspace/executeCommand request')
    end
  end
  if Eshvi and Eshvi.picker then
    local items = {}
    for _, item in ipairs(params.items) do
      table.insert(items, { text = tostring(item.label or ''), desc = item.description, item = item })
    end
    Eshvi.picker.pick({
      title = params.placeholder or 'Select',
      items = items,
      on_choice = function(picked)
        choose(picked and picked.item)
      end,
    })
  else
    vim.ui.select(params.items, {
      prompt = params.placeholder,
      kind = 'slang.quickPick',
      format_item = function(item)
        return item.description and (item.label .. ' ' .. item.description) or item.label
      end,
    }, choose)
  end
end

-- ---- Asking the server ----

-- The commands a client advertises.
local function supports(client, command)
  local provider = client.server_capabilities and client.server_capabilities.executeCommandProvider
  return provider ~= nil and vim.tbl_contains(provider.commands or {}, command)
end

-- `workspace/executeCommand` on slang-server, after checking that it is attached and has the command
-- (as slang-server.nvim did). `done(result)` on success; a failure is a notification.
local function call(command, arguments, done)
  local client = hierarchy.find_client()
  if not client then
    error_message('slang-server: no slang-server LSP client attached')
    return
  end
  if not supports(client, command) then
    error_message(("slang-server: server does not support LSP command '%s'. Please upgrade slang-server."):format(command))
    return
  end
  hierarchy.execute(command, arguments, function(err, result)
    if err then
      error_message(err)
    elseif done then
      done(result)
    end
  end)
end

-- ---- Search ----

-- The hierarchy search: `slang.searchHierarchy` per query (after `QUERY_DELAY`, one in flight and the
-- newest query next), the matches in the native picker, the choice revealed in the Hierarchy view.
function M.search()
  if not (Eshvi and Eshvi.picker) then
    error_message('The hierarchy search needs the native picker')
    return
  end
  local client = hierarchy.find_client()
  if not client then
    error_message('slang-server: no slang-server LSP client attached')
    return
  end
  if not supports(client, 'slang.searchHierarchy') then
    error_message("slang-server: server does not support LSP command 'slang.searchHierarchy'. Please upgrade slang-server.")
    return
  end
  M._search_generation = (M._search_generation or 0) + 1
  local search_generation = M._search_generation
  local request_id = 0
  local in_flight = false
  local pending

  local function dispatch()
    if in_flight or not pending or M._search_generation ~= search_generation then
      return
    end
    local request = pending
    pending = nil
    in_flight = true
    hierarchy.execute('slang.searchHierarchy', { request.query }, function(err, result)
      in_flight = false
      if M._search_generation ~= search_generation then
        return
      end
      if request.id == request_id then
        if err then
          error_message(err)
        else
          local items = {}
          for _, match in ipairs(type(result) == 'table' and result.matches or {}) do
            table.insert(items, { text = match.path, desc = match.description, match = match })
          end
          request.reply(items)
        end
      end
      dispatch()
    end)
  end

  Eshvi.picker.pick({
    title = 'Search hierarchy',
    hint = 'Type to search the hierarchy',
    ask = function(query, reply)
      request_id = request_id + 1
      local mine = request_id
      vim.defer_fn(function()
        if M._search_generation ~= search_generation or mine ~= request_id then
          return
        end
        pending = { id = mine, query = query, reply = reply }
        dispatch()
      end, QUERY_DELAY)
    end,
    on_choice = function(item)
      M._search_generation = M._search_generation + 1
      if item then
        hierarchy.reveal(item.match.path, { focus = true })
      end
    end,
  })
end

-- ---- :EshviSlang ----

-- The lens on `line` that selects the active instance: slang-server's `slang.quickPick` with
-- `slang.activateInstance` (what the module header's "path (1)" lens is).
local function is_active_selection_lens(lens)
  local command = lens.command
  local params = command and command.arguments and command.arguments[1]
  return command ~= nil
    and command.command == 'slang.quickPick'
    and type(params) == 'table'
    and params.onSelectCommand == 'slang.activateInstance'
    and params.interactionSource == 'codeLensSelect'
end

-- The active-instance code lens on the cursor's line, run: its quick pick. Asked of the server
-- (`textDocument/codeLens`) rather than read from the buffer's lenses, so it works with code lenses off.
function M.select_active()
  local client = hierarchy.find_client()
  if not client then
    error_message('slang-server: no slang-server LSP client attached')
    return
  end
  if not supports(client, 'slang.activateInstance') then
    error_message("slang-server: server does not support LSP command 'slang.activateInstance'. Please upgrade slang-server.")
    return
  end
  hierarchy.to_editor_window()
  local bufnr = vim.api.nvim_get_current_buf()
  if not (client.attached_buffers and client.attached_buffers[bufnr]) then
    error_message('slang-server: the current buffer has no slang-server LSP client attached')
    return
  end
  local line = vim.api.nvim_win_get_cursor(0)[1] - 1
  client:request('textDocument/codeLens', { textDocument = vim.lsp.util.make_text_document_params(bufnr) }, function(err, lenses)
    if err then
      error_message(err.message)
      return
    end
    local matches = {}
    for _, lens in ipairs(lenses or {}) do
      if lens.range.start.line == line and is_active_selection_lens(lens) then
        table.insert(matches, lens)
      end
    end
    local function run(lens)
      if lens then
        client:exec_cmd(lens.command, { bufnr = bufnr })
      end
    end
    if #matches == 0 then
      vim.notify('No active-selection code lens found on the current line', vim.log.levels.WARN)
    elseif #matches == 1 then
      run(matches[1])
    else
      vim.ui.select(matches, {
        prompt = 'Select active-selection code lens',
        format_item = function(lens)
          return lens.command.title
        end,
      }, run)
    end
  end, bufnr)
end

-- ---- Up ----

-- The module the cursor is in: of the file's modules (`slang.getModulesInFile`), the last declared at or above the
-- cursor, else the first.
local function module_at_cursor(modules)
  local cursor = vim.api.nvim_win_get_cursor(0)[1]
  local best, best_line = modules[1], 0
  for lnum, line in ipairs(vim.api.nvim_buf_get_lines(0, 0, cursor, false)) do
    for _, name in ipairs(modules) do
      local keyword, after = line:match('^%s*(%a+)%s+(.*)$')
      if keyword == 'module' or keyword == 'macromodule' or keyword == 'interface' or keyword == 'program' then
        after = after:gsub('^automatic%s+', ''):gsub('^static%s+', '')
        if after:match('^' .. vim.pesc(name) .. '[^%w_$]') or after == name then
          best, best_line = name, lnum
        end
      end
    end
  end
  return best
end

-- Go up from the instance at `path` (whose instantiation is `loc`, else the one the hierarchy has): the
-- nearest enclosing instance (generate blocks and arrays skipped) becomes the active one, and the instantiation opens.
local function climb(path, loc)
  hierarchy.chain(path, function(err, nodes, complete)
    if err then
      error_message(err)
      return
    end
    local node = nodes and nodes[#nodes]
    if not (complete and node) then
      error_message('EshviSlang up: ' .. path .. ' is not in the design')
      return
    end
    loc = loc or node.instLoc
    if not (loc and loc.uri) then
      error_message('EshviSlang up: no instantiation found for ' .. path)
      return
    end
    local parent
    for i = #nodes - 1, 1, -1 do
      if nodes[i].kind == 'Instance' then
        parent = nodes[i]
        break
      end
    end
    if not parent then
      vim.notify(path .. ' is a top level: there is nothing above it', vim.log.levels.WARN)
      return
    end
    hierarchy.set_active(parent.path, function(err2)
      if err2 then
        error_message(err2)
        return
      end
      hierarchy.goto_location(loc)
      if hierarchy.registered() then
        hierarchy.reveal(parent.path, { select = true, show = false })
      end
    end)
  end)
end

-- From a file of the active instance's module: its instantiation opens and the instance around it is active.
-- When the active instance isn't of this file's module, which instance of the module is asked
-- (`slang.getInstancesOfModule`, in the native picker).
function M.up()
  local client = hierarchy.find_client()
  if not client then
    error_message('slang-server: no slang-server LSP client attached')
    return
  end
  hierarchy.to_editor_window()
  local file = vim.fs.normalize(vim.api.nvim_buf_get_name(0))
  if file == '' then
    error_message('EshviSlang up: a buffer with a file')
    return
  end
  local function ask()
    for _, command in ipairs({ 'slang.getModulesInFile', 'slang.getInstancesOfModule' }) do
      if not supports(client, command) then
        error_message(("slang-server: server does not support LSP command '%s'. Please upgrade slang-server."):format(command))
        return
      end
    end
    call('slang.getModulesInFile', { file }, function(modules)
      modules = type(modules) == 'table' and modules or {}
      local module = module_at_cursor(modules)
      if not module then
        vim.notify('No module in this file', vim.log.levels.WARN)
        return
      end
      call('slang.getInstancesOfModule', { module }, function(instances)
        local list = {}
        for _, item in ipairs(type(instances) == 'table' and instances or {}) do
          if type(item) == 'table' and type(item.instPath) == 'string' and item.instPath ~= '' then
            table.insert(list, item)
          end
        end
        if #list == 0 then
          vim.notify('No instances of ' .. module, vim.log.levels.WARN)
        elseif #list == 1 then
          climb(list[1].instPath, list[1].instLoc)
        elseif Eshvi and Eshvi.picker then
          local items = {}
          for _, item in ipairs(list) do
            table.insert(items, { text = item.instPath, item = item })
          end
          Eshvi.picker.pick({
            title = 'Go up from which instance?',
            items = items,
            on_choice = function(picked)
              if picked then
                climb(picked.item.instPath, picked.item.instLoc)
              end
            end,
          })
        else
          vim.ui.select(list, {
            prompt = 'Go up from which instance?',
            format_item = function(item)
              return item.instPath
            end,
          }, function(item)
            if item then
              climb(item.instPath, item.instLoc)
            end
          end)
        end
      end)
    end)
  end
  local active = hierarchy.active_path()
  if not active then
    ask()
    return
  end
  hierarchy.chain(active, function(err, nodes, complete)
    local decl = not err and complete and nodes[#nodes] and nodes[#nodes].declLoc
    if decl and decl.uri and vim.fs.normalize(vim.uri_to_fname(decl.uri)) == file then
      climb(active)
    else
      ask()
    end
  end)
end

-- The folder a relative file argument is in: slang-server's workspace (where `server.json` and the build
-- files are, and what the server resolves a relative top file against), else Neovim's. Not Neovim's first:
-- typed from a subfolder, `.slang/x.f` named a file that wasn't there, which the server took without an error.
local function workspace()
  local client = hierarchy.find_client()
  local root = client and (client.root_dir or (client.config and client.config.root_dir))
  return type(root) == 'string' and root ~= '' and root or vim.fn.getcwd()
end

local function is_absolute(path)
  return path:sub(1, 1) == '/' or path:match('^%a:[/\\]') ~= nil
end

-- A file argument as an absolute path, relative ones in the workspace.
local function absolute(file)
  local path = vim.fn.expand(file)
  if not is_absolute(path) then
    path = workspace() .. '/' .. path
  end
  return vim.fs.normalize(path)
end

-- The file `setTopLevel` and `setBuildFile` take: the argument (which must be there: the server takes a missing
-- one without a word and is left with no design), else (if `current`) the buffer's. Nil and why when there is none.
-- (No server is said first: without one there is no workspace to read a relative file in.)
local function file_of(args, current)
  if not hierarchy.find_client() then
    return nil, 'no slang-server LSP client attached'
  end
  if args[1] then
    local file = absolute(args[1])
    if vim.fn.filereadable(file) == 0 then
      return nil, 'no such file: ' .. file
    end
    return file
  end
  if current then
    hierarchy.to_editor_window()
    local name = vim.api.nvim_buf_get_name(0)
    return name ~= '' and name or nil
  end
end

-- Completion of a file argument, relative ones in the workspace (as `absolute` reads them): folders end in `/`.
local function complete_file(lead)
  local expanded = vim.fn.expand(lead)
  if is_absolute(expanded) or lead:sub(1, 1) == '~' then
    return vim.fn.getcompletion(lead, 'file')
  end
  local root = workspace()
  local out = {}
  for _, path in ipairs(vim.fn.glob(root .. '/' .. lead .. '*', false, true)) do
    local rel = path:sub(#root + 2)
    if vim.fn.isdirectory(path) == 1 then
      rel = rel .. '/'
    end
    table.insert(out, rel)
  end
  table.sort(out)
  return out
end

-- The signal trace commands: need the view (a slang-server or svtrace client attached).
local function traced(run)
  return function()
    if not run() then
      error_message('EshviSlang: this needs an attached slang-server or svtrace')
    end
  end
end

-- name -> { desc, run(args), files = true | list of completions }
local subcommands = {
  hierarchy = {
    desc = 'Show the design hierarchy (and the active instance)',
    run = function()
      if not hierarchy.open() then
        error_message('EshviSlang: this needs an attached slang-server or svtrace')
      end
    end,
  },
  search = { desc = 'Search the compiled design hierarchy', run = M.search },
  drivers = {
    desc = 'Trace the drivers of the signal under the cursor',
    run = traced(function()
      return trace.trace('drivers')
    end),
  },
  loads = {
    desc = 'Trace the loads of the signal under the cursor',
    run = traced(function()
      return trace.trace('loads')
    end),
  },
  driverEndpoints = {
    desc = 'Where the signal under the cursor is assigned, past the wiring',
    run = traced(function()
      return trace.endpoints('drivers')
    end),
  },
  loadEndpoints = {
    desc = 'Where the signal under the cursor is read, past the wiring',
    run = traced(function()
      return trace.endpoints('loads')
    end),
  },
  selectActive = {
    desc = 'Select the active instance or generate iteration under the cursor',
    run = function()
      M.select_active()
    end,
  },
  up = {
    desc = 'Go up to where the active instance is instantiated, and make the instance around it active',
    run = function()
      M.up()
    end,
  },
  setTopLevel = {
    desc = 'Set the current design top level',
    files = true,
    run = function(args)
      local file, why = file_of(args, true)
      if not file then
        error_message('EshviSlang setTopLevel: ' .. (why or 'a file, or a buffer with one'))
        return
      end
      call('slang.setTopLevel', { file })
    end,
  },
  setBuildFile = {
    desc = 'Set the Slang Server build file',
    files = true,
    run = function(args)
      local file, why = file_of(args, false)
      if not file then
        error_message('EshviSlang setBuildFile: ' .. (why or 'a file'))
        return
      end
      call('slang.setBuildFile', { file })
    end,
  },
  addToWaves = {
    desc = 'Add the object under the cursor to the waveform',
    files = { 'true', 'false' },
    run = function(args)
      local client = hierarchy.find_client()
      if not client then
        error_message('slang-server: no slang-server LSP client attached')
        return
      end
      if not (supports(client, 'slang.getInstances') and supports(client, 'slang.addToWaveform')) then
        error_message("slang-server: server does not support LSP commands 'slang.getInstances' and 'slang.addToWaveform'. Please upgrade slang-server.")
        return
      end
      hierarchy.to_editor_window()
      local recursive = args[1] == 'true'
      local params = vim.lsp.util.make_position_params(0, client.offset_encoding or 'utf-16')
      call('slang.getInstances', { params }, function(paths)
        local list = {}
        for _, item in ipairs(type(paths) == 'table' and paths or {}) do
          local path = type(item) == 'string' and item or (type(item) == 'table' and (item.path or item.hierPath)) or nil
          if type(path) == 'string' and path ~= '' then
            table.insert(list, path)
          end
        end
        local function add(path)
          call('slang.addToWaveform', { { path = path, recursive = recursive } })
        end
        if #list == 0 then
          vim.notify('No instances to add', vim.log.levels.WARN)
        elseif #list == 1 then
          add(list[1])
        else
          if Eshvi and Eshvi.picker then
            local items = {}
            for _, path in ipairs(list) do
              table.insert(items, { text = path })
            end
            Eshvi.picker.pick({
              title = 'Add instance to waves',
              items = items,
              on_choice = function(item)
                if item then
                  add(item.text)
                end
              end,
            })
          else
            vim.ui.select(list, { prompt = 'Add instance to waves' }, function(path)
              if path then
                add(path)
              end
            end)
          end
        end
      end)
    end,
  },
  openWaveform = {
    desc = 'Open a waveform file',
    files = true,
    run = function(args)
      local file, why = file_of(args, false)
      if not file then
        error_message('EshviSlang openWaveform: ' .. (why or 'a file'))
        return
      end
      call('slang.openWaveform', { file })
    end,
  },
}

-- In the order of the spec (and of the completion).
local names = {
  'hierarchy', 'search', 'drivers', 'loads', 'driverEndpoints', 'loadEndpoints', 'selectActive', 'up',
  'setTopLevel', 'setBuildFile', 'addToWaves', 'openWaveform',
}

local function with_prefix(list, lead)
  return vim.tbl_filter(function(name)
    return vim.startswith(name, lead)
  end, list)
end

local function create_command()
  vim.api.nvim_create_user_command('EshviSlang', function(o)
    local sub = subcommands[o.fargs[1]]
    if not sub then
      error_message('EshviSlang: ' .. table.concat(names, ', '))
      return
    end
    sub.run(vim.list_slice(o.fargs, 2))
  end, {
    nargs = '+',
    desc = 'eshvi slang: hierarchy, trace and server commands',
    complete = function(lead, line)
      -- The words before the one being completed, the command itself first.
      local words = vim.split(line, '%s+', { trimempty = true })
      local done = #words - (line:match('%s$') and 0 or 1)
      if done <= 1 then
        return with_prefix(names, lead)
      end
      local sub = subcommands[words[2]]
      if done == 2 and sub and sub.files == true then
        return complete_file(lead)
      elseif done == 2 and sub and type(sub.files) == 'table' then
        return with_prefix(sub.files, lead)
      end
      return {}
    end,
  })
end

-- ---- svtrace's language server ----

-- svtrace answers the signal trace and the hierarchy's scopes (crates/svtrace). It ships beside the app with
-- the slang it runs (`$ESHVI_BIN`), and starts when both are there. A `cmd` the config set for `svtrace`
-- wins: it keeps the development pair in `~/tools` and enables it itself.
-- Config or app first gives the same: the app's settings are the defaults that a later `vim.lsp.config` call
-- replaces, and the app enables svtrace only once it is needed (the first Verilog buffer: the config has run
-- by then, even a lazy one that loads on BufReadPre) and only if the `cmd` is still its own.
local function setup_svtrace()
  local bin = vim.env.ESHVI_BIN or ''
  local existing = vim.lsp.config[SVTRACE]
  if bin == '' or (existing and existing.cmd) then
    return
  end
  local svtrace, slang = bin .. '/svtrace', bin .. '/slang'
  if vim.fn.executable(svtrace) ~= 1 or vim.fn.executable(slang) ~= 1 then
    return
  end
  local defaults = { cmd = { svtrace, '--slang', slang }, root_markers = { '.slang' }, filetypes = { 'systemverilog', 'verilog' } }
  -- (Only what the config hasn't given, so it can set its own filetypes without a cmd.)
  local missing = {}
  for key, value in pairs(defaults) do
    if existing == nil or existing[key] == nil then
      missing[key] = value
    end
  end
  vim.lsp.config(SVTRACE, missing)

  local function enable()
    local now = vim.lsp.config[SVTRACE]
    if now and vim.deep_equal(now.cmd, defaults.cmd) then
      vim.lsp.enable(SVTRACE)
    end
  end
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if vim.tbl_contains(defaults.filetypes, vim.bo[buf].filetype) then
      enable()
      return
    end
  end
  vim.api.nvim_create_autocmd('FileType', {
    group = vim.api.nvim_create_augroup('eshvi_slang_svtrace', { clear = true }),
    pattern = defaults.filetypes,
    once = true,
    callback = enable,
  })
end

local started = false

function M.setup()
  if started or vim.g.eshvi_slang == false then
    return
  end
  started = true
  vim.lsp.commands = vim.lsp.commands or {}
  vim.lsp.commands['slang.quickPick'] = quick_pick
  hierarchy.install()
  watch_clients()
  create_command()
  setup_svtrace()
end

return M
