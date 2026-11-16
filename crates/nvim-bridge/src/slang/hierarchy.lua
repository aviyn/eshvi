-- The design hierarchy as a view of its own in Eshvi's side bar, drawn by the app (`Eshvi.tree`,
-- NOTES.md "Outline and the native tree view"). Part of the app (`eshvi.slang.hierarchy`, set up by
-- `eshvi.slang`); everything slang-specific is here, the app knows only the generic tree.
--
-- It talks to the server directly (`workspace/executeCommand`: slang.getScope, slang.getScopes,
-- slang.setActiveInstance, slang.showHierLocation). The two scope commands go to svtrace instead when it
-- is attached and advertises them; the rest stays with slang-server. Outside Eshvi (no `Eshvi.tree`) it
-- does nothing.
--
--   M.register() / M.unregister()         the view (`eshvi.slang` calls them as a server attaches and goes)
--   M.reveal(path, { focus, select })     show the view and the node at a hierarchy path
--   M.open()                              show the view with the keyboard (and the active instance)
--   M.refresh()                           roots again, then each loaded node's children
--   M.install()                           the server's own requests (`slang.showInHierarchy`, `slang/activeInstanceChanged`)
--
-- Shared with the signal trace (`trace.lua`) and `eshvi.slang`: SERVER, execute, fail, find_client,
-- any_attached, active_path, to_editor_window, goto_location, yank, decoration and color_of.
local M = {}

local VIEW = 'slang'
local SERVER = 'slang-server'

local registered = false
local reveal_generation = 0
local load_generation = 0
-- Nodes whose children were pushed, by id, with their path and depth, for a refresh.
local known = {}
-- The active instance by client id (what `slang/activeInstanceChanged` and `slang.showInHierarchy` last said).
M.active_paths = {}

function M.available()
  return rawget(_G, 'Eshvi') ~= nil and Eshvi.tree ~= nil
end

-- ---- Kinds, icons and colors ----

local DEFAULT_KINDS = {
  instance = { icon = '\u{ec19}', hl = 'SlangServerInstance' },
  instancearray = { icon = '\u{ec19}', hl = 'SlangServerInstanceArray' },
  scope = { icon = '\u{f0169}', hl = 'SlangServerScope' },
  scopearray = { icon = '\u{f0169}', hl = 'SlangServerScopeArray' },
  interfaceport = { icon = '\u{f0200}', hl = 'SlangServerInterfacePort' },
  interfaceportarray = { icon = '\u{f0200}', hl = 'SlangServerInterfacePortArray' },
  package = { icon = '\u{1f4e6}', hl = 'SlangServerPackage' },
  port = {
    input = { icon = '\u{f061}', hl = 'SlangServerPortInput' },
    output = { icon = '\u{f060}', hl = 'SlangServerPortOutput' },
    inout = { icon = '\u{f0ec}', hl = 'SlangServerPortInout' },
  },
  param = { icon = '\u{e22c}', hl = 'SlangServerParam' },
  logic = { icon = '\u{f1496}', hl = 'SlangServerLogic' },
  reg = { icon = '\u{f074}', hl = 'SlangServerReg' },
}

local DEFAULT_COLORS = {
  SlangServerInstance = '#efbd5d',
  SlangServerInstanceArray = '#efbd5d',
  SlangServerScope = '#41a7fc',
  SlangServerScopeArray = '#41a7fc',
  SlangServerInterfacePort = '#34bfd0',
  SlangServerInterfacePortArray = '#34bfd0',
  SlangServerPackage = '#f48fb1',
  SlangServerPortInput = '#8bcd5b',
  SlangServerPortOutput = '#f65866',
  SlangServerPortInout = '#34bfd0',
  SlangServerParam = '#c75ae8',
  SlangServerLogic = '#dd9046',
  SlangServerReg = '#dd9046',
}

-- The kinds the tree can open.
local EXPANDABLE = {
  Instance = true,
  Scope = true,
  InstanceArray = true,
  ScopeArray = true,
  InterfacePort = true,
  InterfacePortArray = true,
  Package = true,
}

-- What the header's Filter menu toggles, in its order, and the kinds under each. A
-- kind that isn't here has no toggle and is always shown.
local FILTERS = {
  { id = 'instance', label = 'Instances', kinds = { 'Instance', 'InstanceArray' } },
  { id = 'scope', label = 'Scopes', kinds = { 'Scope', 'ScopeArray' } },
  { id = 'interface', label = 'Interface Ports', kinds = { 'InterfacePort', 'InterfacePortArray' } },
  { id = 'package', label = 'Packages', kinds = { 'Package' } },
  { id = 'port', label = 'Ports', kinds = { 'Port' } },
  { id = 'param', label = 'Parameters', kinds = { 'Param' } },
  { id = 'signal', label = 'Signals', kinds = { 'Logic', 'Reg' } },
}

local GROUP_OF = {}
for _, filter in ipairs(FILTERS) do
  for _, kind in ipairs(filter.kinds) do
    GROUP_OF[kind] = filter.id
  end
end

local function decoration(kind)
  return DEFAULT_KINDS[string.lower(kind or '')]
end

-- The color of a highlight group as a number: the colorscheme's if it defines the group
-- (`SlangServerInstance`, `SlangServerPortInput`...), else the default.
local function color_of(group)
  if not group then
    return nil
  end
  local ok, hl = pcall(vim.api.nvim_get_hl, 0, { name = group, link = false })
  if ok and hl.fg then
    return hl.fg
  end
  local hex = (DEFAULT_COLORS[group] or ''):match('^#(%x%x%x%x%x%x)$')
  return hex and tonumber(hex, 16) or nil
end

-- The path rules: `inst[0]` and `pkg::x` are subtle (the rules slang-server.nvim's `navigation.path` had).
local path_rules = {}

function path_rules.join(parent_path, child_name, parent_kind)
  if parent_path == '' then
    return child_name
  elseif child_name:sub(1, 1) == '[' then
    return parent_path .. child_name
  elseif parent_kind == 'Package' then
    return parent_path .. '::' .. child_name
  end
  return parent_path .. '.' .. child_name
end

function path_rules.split(path)
  local parts = {}
  local current = ''
  local index = 1
  while index <= #path do
    local char = path:sub(index, index)
    local next_char = path:sub(index + 1, index + 1)
    if char == '.' or (char == ':' and next_char == ':') then
      if current ~= '' then
        parts[#parts + 1] = current
        current = ''
      end
      index = index + (char == ':' and 2 or 1)
    else
      if char == '[' and current ~= '' then
        parts[#parts + 1] = current
        current = ''
      end
      current = current .. char
      if char == ']' then
        parts[#parts + 1] = current
        current = ''
      end
      index = index + 1
    end
  end
  if current ~= '' then
    parts[#parts + 1] = current
  end
  return parts
end

function path_rules.resolve_child(children, parts, index, last_path)
  local part = parts[index]
  if not part then
    return nil, index
  end
  for _, child in ipairs(children) do
    if child.instName == part then
      return child, index
    end
  end
  local next_part = parts[index + 1]
  if next_part and next_part:sub(1, 1) == '[' then
    for _, child in ipairs(children) do
      if child.instName == part .. next_part then
        return child, index + 1
      end
    end
  end
  if part:sub(1, 1) == '[' and last_path then
    for _, child in ipairs(children) do
      if child.path == last_path .. part then
        return child, index
      end
    end
  end
  return nil, index
end

-- ---- The server ----

-- The running clients of a server, by its `server_info.name` (slang-server unless given).
local function slang_clients(name)
  name = name or SERVER
  local out = {}
  for _, client in ipairs(vim.lsp.get_clients()) do
    if client.server_info and client.server_info.name == name and not client:is_stopped() then
      table.insert(out, client)
    end
  end
  return out
end

-- A client of the server (slang-server unless named) and a buffer it is attached to (the current one if it is).
local function find_client(name)
  local current = vim.api.nvim_get_current_buf()
  local first
  for _, client in ipairs(slang_clients(name)) do
    if client.attached_buffers and client.attached_buffers[current] then
      return client, current
    end
    if not first then
      for buf in pairs(client.attached_buffers or {}) do
        if vim.api.nvim_buf_is_valid(buf) then
          first = { client, buf }
          break
        end
      end
    end
  end
  if first then
    return first[1], first[2]
  end
end

-- `workspace/executeCommand` on the server (slang-server unless named): `done(err_message, result)` once.
local function execute(command, arguments, done, name)
  name = name or SERVER
  local client, bufnr = find_client(name)
  if not client then
    done(('%s: no %s LSP client attached'):format(name, name))
    return
  end
  local finished = false
  local sent = client:request('workspace/executeCommand', { command = command, arguments = arguments }, function(err, result)
    if finished then
      return
    end
    finished = true
    if err then
      done(type(err) == 'table' and err.message or tostring(err))
    else
      done(nil, result)
    end
  end, bufnr)
  if sent == false and not finished then
    finished = true
    done(name .. ': failed to send workspace/executeCommand request')
  end
end

-- The scope commands (`slang.getScope`, `slang.getScopes`) go to svtrace when it is attached and says it has them
-- (it reads the hierarchy from slang's elaborated AST, crates/svtrace/src/hier.rs), else to slang-server.
-- Everything else (`setActiveInstance`, `showHierLocation`, the search) stays with slang-server.
local SVTRACE = 'svtrace'

local function scope_execute(command, arguments, done)
  local client = find_client(SVTRACE)
  local provider = client and client.server_capabilities and client.server_capabilities.executeCommandProvider
  local by_svtrace = provider ~= nil and vim.tbl_contains(provider.commands or {}, command)
  if not by_svtrace then
    execute(command, arguments, done)
    return
  end
  -- (svtrace without a design, as with a build it doesn't support yet: slang-server may still have one.)
  execute(command, arguments, function(err, result)
    if err and tostring(err):find('svtrace: no design', 1, true) then
      execute(command, arguments, done)
    else
      done(err, result)
    end
  end, SVTRACE)
end

local function fail(message)
  vim.notify(tostring(message), vim.log.levels.ERROR)
end

-- ---- Nodes ----

local function text(value)
  return type(value) == 'string' and value ~= '' and value or nil
end

-- The nodes of a server answer under `parent` (a node of ours, or nil for the roots).
local function make_nodes(raw_nodes, parent)
  local P = path_rules
  local nodes = {}
  for _, raw in ipairs(raw_nodes or {}) do
    local node = {}
    for key, value in pairs(raw) do
      if key ~= 'children' then
        node[key] = value
      end
    end
    local name = tostring(raw.instName or '')
    node.path = P.join(parent and parent.path or '', name, parent and parent.kind or nil)
    node.id = node.path
    node.label = name
    local kind = raw.kind
    if (kind == 'Instance' or kind == 'InstanceArray') and text(raw.declName) then
      node.hint = raw.declName
    else
      node.hint = text(raw.type)
    end
    local dec = decoration(kind)
    if kind == 'Port' and dec then
      local type_ = raw.type or ''
      dec = (type_:find('^input') and dec.input) or (type_:find('^output') and dec.output) or dec.inout
    end
    if dec and dec.icon then
      node.icon = dec.icon
      node.hl = dec.hl
      node.fg = color_of(dec.hl)
    else
      node.icon = '?'
    end
    node.expandable = EXPANDABLE[kind] or nil
    node.group = GROUP_OF[kind]
    if kind == 'Param' and text(raw.value) then
      node.tooltip = name .. ' = ' .. raw.value
    else
      node.tooltip = text(raw.type)
    end
    if type(raw.children) == 'table' and #raw.children > 0 then
      node.children = make_nodes(raw.children, node)
    end
    table.insert(nodes, node)
  end
  return nodes
end

local function depth_of(path)
  return #path_rules.split(path)
end

-- Remember that the children of `node` are known, for a refresh. (Its kind: a
-- package's children join to its path with `::`.)
local function remember(node)
  if node then
    known[node.id] = { path = node.path, kind = node.kind, depth = depth_of(node.path) }
  end
end

local function remember_inline(nodes)
  for _, node in ipairs(nodes) do
    if node.children and #node.children > 0 then
      remember(node)
      remember_inline(node.children)
    end
  end
end

-- ---- Loading ----

local function set_roots(nodes, message)
  known = {}
  remember_inline(nodes)
  Eshvi.tree.set(VIEW, nodes, { message = message, expand = 1 })
end

-- The roots, as for an empty top: the first step of
-- `slang.getScopes('')`. Retries while the design isn't loaded yet.
local function load_roots(retries)
  load_generation = load_generation + 1
  local mine = load_generation
  local function try(left)
    scope_execute('slang.getScopes', { '' }, function(err, scopes)
      if mine ~= load_generation or not registered then
        return
      end
      if err then
        set_roots({}, 'No design: ' .. tostring(err))
        if left > 0 then
          vim.defer_fn(function()
            if mine == load_generation and registered then
              try(left - 1)
            end
          end, 1500)
        end
        return
      end
      local step = type(scopes) == 'table' and scopes[1] or nil
      local nodes = make_nodes(step and step.children or {}, nil)
      set_roots(nodes, #nodes == 0 and 'No design' or nil)
    end)
  end
  try(retries or 0)
end

local function children_of(node, reply)
  scope_execute('slang.getScope', { node.path }, function(err, resp)
    if err then
      fail(err)
      reply({})
      return
    end
    local children = make_nodes(type(resp) == 'table' and resp or {}, node)
    remember(node)
    remember_inline(children)
    reply(children)
  end)
end

-- The roots again, then the children of each node that is open as drawn (`ctx.expanded`, from the app)
-- and was loaded, pushed with `update`: what is open stays open. A node that was loaded but is closed
-- gets its children dropped (an update with none: it loads them again, lazily, when next opened).
function M.refresh(ctx)
  if not (registered and M.available()) then
    return
  end
  local expanded = {}
  for _, id in ipairs(ctx and ctx.expanded or {}) do
    expanded[id] = true
  end
  load_generation = load_generation + 1
  local mine = load_generation
  scope_execute('slang.getScopes', { '' }, function(err, scopes)
    if mine ~= load_generation or not registered then
      return
    end
    if err then
      set_roots({}, 'No design: ' .. tostring(err))
      return
    end
    local step = type(scopes) == 'table' and scopes[1] or nil
    local roots = make_nodes(step and step.children or {}, nil)
    if #roots == 0 then
      set_roots({}, 'No design')
      return
    end
    local again = {}
    for id, k in pairs(known) do
      table.insert(again, { id = id, path = k.path, kind = k.kind, depth = k.depth })
    end
    table.sort(again, function(a, b)
      if a.depth ~= b.depth then
        return a.depth < b.depth
      end
      return a.id < b.id
    end)
    remember_inline(roots)
    Eshvi.tree.update(VIEW, nil, roots)
    for _, k in ipairs(again) do
      if expanded[k.id] then
        scope_execute('slang.getScope', { k.path }, function(err2, resp)
          if err2 or mine ~= load_generation or not registered then
            return
          end
          local nodes = make_nodes(type(resp) == 'table' and resp or {}, { path = k.path, kind = k.kind })
          remember_inline(nodes)
          Eshvi.tree.update(VIEW, k.id, nodes)
        end)
      else
        known[k.id] = nil
        Eshvi.tree.update(VIEW, k.id, {})
      end
    end
  end)
end

-- ---- Reveal ----

-- Show the view (unless `show = false`) and the node at `path`: the levels from the root down pushed
-- with `update`, then the last one found selected. A path that doesn't resolve
-- selects the deepest one found and warns. A newer reveal cancels an older.
function M.reveal(path, opts)
  opts = opts or {}
  if not (registered and M.available()) then
    return
  end
  -- (A reveal the server starts passes `show = false`: the side bar stays as it is, the selection is ready.)
  if opts.show ~= false then
    Eshvi.tree.show(VIEW, { focus = opts.focus or false })
  end
  reveal_generation = reveal_generation + 1
  local mine = reveal_generation
  scope_execute('slang.getScopes', { path }, function(err, scopes)
    if mine ~= reveal_generation or not registered then
      return
    end
    if err then
      fail(err)
      return
    end
    scopes = type(scopes) == 'table' and scopes or {}
    local P = path_rules
    local parts = P.split(path)
    local current, last_resolved
    local index = 1

    local function push(parent, step)
      local nodes = make_nodes(step.children, parent)
      remember(parent)
      remember_inline(nodes)
      Eshvi.tree.update(VIEW, parent and parent.id or nil, nodes)
      return nodes
    end

    while index <= #parts do
      local step = scopes[index]
      if not step then
        break
      end
      local children = push(current, step)
      local child, resolved = P.resolve_child(children, parts, index, last_resolved and last_resolved.path or nil)
      if not child then
        break
      end
      last_resolved, current = child, child
      index = resolved + 1
    end

    local final_step = scopes[index]
    if last_resolved and index > #parts and final_step then
      push(last_resolved, final_step)
    elseif #parts == 0 and scopes[1] then
      push(nil, scopes[1])
    end

    if index <= #parts then
      vim.notify(
        string.format(
          "Could not resolve hierarchy path '%s' at '%s' (last resolved: '%s')",
          path,
          parts[index],
          last_resolved and last_resolved.path or 'root'
        ),
        vim.log.levels.WARN
      )
    end
    if opts.select ~= false and last_resolved then
      Eshvi.tree.select(VIEW, last_resolved.id)
    end
  end)
end

-- ---- The active instance, accepting, the menu ----

local function active_path()
  local client = find_client()
  if not client then
    return nil
  end
  return M.active_paths[client.id]
end

-- The window a location opens in: the current one unless it is a panel, a terminal
-- or a float, then the editor's.
local function to_editor_window()
  local win = vim.api.nvim_get_current_win()
  local buf = vim.api.nvim_win_get_buf(win)
  local panel = vim.api.nvim_win_get_config(win).relative ~= ''
    or vim.bo[buf].buftype ~= ''
    or vim.wo[win].winfixwidth
    or vim.wo[win].winfixheight
  if panel and Eshvi.editor_window then
    local editor = Eshvi.editor_window()
    if editor then
      vim.api.nvim_set_current_win(editor)
    end
  end
end

-- Make `node` the active instance, then open its place. An instance opens its own module (`declLoc`, the
-- module's definition: the way in); `instLoc` is where it is instantiated, in the parent (the menu's
-- "Go to Instantiation"). Anything else has the server open its place (a `window/showDocument` into the
-- editor's window, taking the focus there).
local function accept(node)
  to_editor_window()
  M.set_active(node.path, function(err)
    if err then
      fail(err)
      return
    end
    if node.kind == 'Instance' and node.declLoc and node.declLoc.uri then
      M.goto_location(node.declLoc) -- (defined below)
      return
    end
    to_editor_window()
    execute('slang.showHierLocation', { { hierPath = node.path, takeFocus = true } }, function(err2)
      if err2 then
        fail(err2)
      end
    end)
  end)
end

local function yank(str)
  if str and str ~= '' then
    vim.fn.setreg('+', str)
    vim.notify('Yanked ' .. str, vim.log.levels.INFO)
  end
end

-- Open an LSP location ({ uri, range }) in the editor's window, the jumplist marked.
local function goto_location(loc)
  if not (loc and loc.uri) then
    return
  end
  to_editor_window()
  vim.cmd("normal! m'")
  local target = vim.fs.normalize(vim.uri_to_fname(loc.uri))
  local current = vim.fs.normalize(vim.api.nvim_buf_get_name(0))
  if target ~= current then
    -- (A swap file left by a crash is E325: the file opens all the same, so only a short note, not a stack trace.)
    local ok, err = pcall(vim.cmd.edit, vim.fn.fnameescape(target))
    if not ok then
      local opened = vim.fs.normalize(vim.api.nvim_buf_get_name(0)) == target
      local message = tostring(err):match('(E%d+:.*)$') or tostring(err)
      vim.notify(opened and (target .. ': ' .. message:gsub('\n.*', '')) or message, vim.log.levels.WARN)
      if not opened then
        return
      end
    end
  end
  local start = loc.range and loc.range.start
  if start then
    pcall(vim.api.nvim_win_set_cursor, 0, { start.line + 1, start.character })
  end
  vim.cmd('normal! zv')
end

local function goto_declaration(node)
  goto_location(node.declLoc)
end

local function goto_instantiation(node)
  goto_location(node.instLoc)
end

local function menu(node)
  local items = {
    {
      label = 'Copy Path',
      run = function(n)
        yank(n.path)
      end,
    },
  }
  if node.kind == 'Param' and text(node.value) then
    table.insert(items, {
      label = 'Copy Value',
      run = function(n)
        yank(n.value)
      end,
    })
  end
  if node.instLoc and node.instLoc.uri then
    table.insert(items, {
      label = 'Copy File Path',
      run = function(n)
        yank(vim.uri_to_fname(n.instLoc.uri))
      end,
    })
  end
  if node.declLoc and node.declLoc.uri then
    table.insert(items, { label = 'Go to Declaration', run = goto_declaration, editor = true })
  end
  if node.kind == 'Instance' and node.instLoc and node.instLoc.uri then
    table.insert(items, { label = 'Go to Instantiation', run = goto_instantiation, editor = true })
  end
  -- (The signal trace, `trace.lua`, loaded when used: it needs this module.)
  if node.group == 'port' or node.group == 'signal' then
    for _, direction in ipairs({ 'drivers', 'loads' }) do
      table.insert(items, {
        label = direction == 'drivers' and 'Trace Drivers' or 'Trace Loads',
        run = function(n)
          local loc = n.instLoc
          local start = loc and loc.range and loc.range.start
          require('eshvi.slang.trace').trace_path(n.path, direction, {
            location = loc and loc.uri and { uri = loc.uri, line = start and start.line, col = start and start.character } or nil,
          })
        end,
      })
    end
  end
  return items
end

-- ---- Search ----

-- The hierarchy search, in the native picker (`eshvi.slang`'s: it asks `slang.searchHierarchy`); the
-- match is revealed in this view.
function M.search()
  require('eshvi.slang').search()
end

-- `<leader>vh`: the view with the keyboard, and the active instance revealed if there is one.
function M.open()
  if not (registered and M.available()) then
    return false
  end
  Eshvi.tree.show(VIEW, { focus = true })
  local path = active_path()
  if path then
    M.reveal(path, { focus = true })
  end
  return true
end

-- ---- The server's own requests ----

-- Record the active path (`active_path` reads it back: `:EshviSlang hierarchy` and the trace's
-- "prefer" look there).
local function record_active(client_id, path)
  M.active_paths[client_id] = path
end

-- Make `path` the active instance (`slang.setActiveInstance`) and remember it: the server tells of a change only
-- when a lens or its own request makes it, not for this command. `done(err_message)` once.
function M.set_active(path, done)
  execute('slang.setActiveInstance', { path }, function(err)
    local client = find_client()
    if not err and client then
      record_active(client.id, path)
    end
    done(err)
  end)
end

-- The nodes along `path`, from the root: each part resolved as `reveal` does it. `done(err_message, nodes,
-- complete)`; `complete` is false when the design doesn't have the whole path.
function M.chain(path, done)
  scope_execute('slang.getScopes', { path }, function(err, scopes)
    if err then
      done(err)
      return
    end
    scopes = type(scopes) == 'table' and scopes or {}
    local P = path_rules
    local parts = P.split(path)
    local nodes, current = {}, nil
    local index = 1
    while index <= #parts and scopes[index] do
      local children = make_nodes(scopes[index].children, current)
      local child, resolved = P.resolve_child(children, parts, index, current and current.path or nil)
      if not child then
        break
      end
      table.insert(nodes, child)
      current = child
      index = resolved + 1
    end
    done(nil, nodes, index > #parts)
  end)
end

-- The code lenses name the active instance: asked for again, in the buffers that show lenses. (Neovim
-- 0.12's way: `enable` again; `codelens.refresh` is deprecated and warns.)
local function refresh_lenses(client_id)
  local client = vim.lsp.get_client_by_id(client_id)
  if not (client and vim.lsp.codelens and vim.lsp.codelens.enable and vim.lsp.codelens.is_enabled) then
    return
  end
  for bufnr in pairs(client.attached_buffers or {}) do
    if vim.api.nvim_buf_is_loaded(bufnr) and vim.lsp.codelens.is_enabled({ bufnr = bufnr }) then
      vim.lsp.codelens.enable(true, { bufnr = bufnr })
    end
  end
end

-- The server says the active instance changed: recorded, the lenses refreshed, and revealed here without
-- the keyboard and without switching the side bar (the selection is ready when the view is opened).
local function active_changed(err, params, ctx)
  if err or type(params) ~= 'table' or type(params.hierPath) ~= 'string' or not (ctx and ctx.client_id) then
    return
  end
  record_active(ctx.client_id, params.hierPath)
  refresh_lenses(ctx.client_id)
  if registered then
    M.reveal(params.hierPath, { select = true, show = false })
  end
end

local function show_in_hierarchy(command)
  local params = command and command.arguments and command.arguments[1]
  if type(params) ~= 'table' or type(params.hierPath) ~= 'string' then
    fail('slang-server: invalid show in hierarchy command')
    return
  end
  if registered then
    M.reveal(params.hierPath, { select = true, show = false })
  end
end

-- The server's own requests: the code lens "path (1)" on a module header, and the active instance changing.
-- Set again at each attach (cheap), so a config that registers its own after setup (an old slang-server.nvim)
-- doesn't keep these from standing.
function M.install()
  if not M.available() then
    return
  end
  vim.lsp.commands = vim.lsp.commands or {}
  vim.lsp.commands['slang.showInHierarchy'] = show_in_hierarchy
  vim.lsp.handlers['slang/activeInstanceChanged'] = active_changed
end

-- ---- The view ----

function M.register()
  if registered or not M.available() then
    return
  end
  registered = true
  Eshvi.tree.register(VIEW, {
    side = {
      title = 'HIERARCHY',
      icon = 'type-hierarchy',
      tooltip = 'Hierarchy',
      buttons = {
        {
          icon = 'refresh',
          tooltip = 'Refresh',
          run = function(ctx)
            M.refresh(ctx)
          end,
        },
      },
      filters = FILTERS,
    },
    children = children_of,
    accept = function(node)
      accept(node)
    end,
    menu = menu,
    keys = {
      {
        key = '/',
        desc = 'Search',
        run = function()
          M.search()
        end,
      },
      {
        key = 'y',
        desc = 'Copy path',
        run = function(node)
          if node then
            yank(node.path)
          end
        end,
      },
    },
  })
  load_roots(5)
end

function M.unregister()
  if not registered then
    return
  end
  registered = false
  known = {}
  load_generation = load_generation + 1
  reveal_generation = reveal_generation + 1
  Eshvi.tree.unregister(VIEW)
end

-- Whether a slang-server or svtrace client has a buffer: what the views live by.
local function any_attached()
  for _, name in ipairs({ SERVER, SVTRACE }) do
    for _, client in ipairs(slang_clients(name)) do
      if next(client.attached_buffers or {}) then
        return true
      end
    end
  end
  return false
end

M.SERVER = SERVER
M.SVTRACE = SVTRACE
M.execute = execute
M.fail = fail
M.find_client = find_client
M.any_attached = any_attached
M.active_path = active_path
M.to_editor_window = to_editor_window
M.goto_location = goto_location
M.yank = yank
M.decoration = decoration
M.color_of = color_of
M.registered = function()
  return registered
end

return M
