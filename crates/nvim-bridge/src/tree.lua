-- The native tree view's Neovim side: a source gives nodes, the app draws the
-- tree (`tree_view.rs`: rows, expansion, selection, keys) and asks back. Loaded
-- with any config. The Outline (`outline.lua`) is the first source; a config
-- can register its own.
--
--   Eshvi.tree.register(view, {
--     children = function(node, reply) end,  -- lazy children of `node`; reply(nodes) once
--     accept   = function(node, how) end,    -- how: "open" (click, Enter: the keyboard goes to the editor) or "peek" (Space)
--     menu     = function(node) return { { label = 'Copy Path', run = function(node) end }, ... } end,  -- right-click
--   })
--   Eshvi.tree.set(view, nodes, { message = nil, expand = 1, buf = nil })
--                                             the whole root list, children inline where known
--   Eshvi.tree.select(view, id)               select and reveal
--   Eshvi.tree.unregister(view)
--   Eshvi.tree.update(view, parent_id, nodes) push the children of a node (the roots for parent_id nil)
--                                             unasked, merged by id: nodes kept keep their expansion and
--                                             the children they have unless the update brings new ones;
--                                             nodes missing from it go
--   Eshvi.tree.show(view, { focus = false })  switch the side bar to a view that has `side`
--   Eshvi.tree.hide(view)                     back to the Explorer, if it is showing
--
-- A source may also give, in `register`:
--   side = { title = "HIERARCHY", icon = "type-hierarchy" (a codicon's name), tooltip = "Hierarchy",
--            buttons = { { icon = "refresh", tooltip = "Refresh", run = function(ctx) end } },
--            filters = { { id = "port", label = "Ports", hidden = false }, ... } }
--                 `filters`: a Filter button in the header, a menu with a toggle for each. A node gives
--                 `group = "port"` to answer to one (without it, it is always shown). The app hides the
--                 rows, so a source sends every node as always; the choices are remembered per view
--                 (`hidden` is only what a view starts with before there is a choice)
--                 `ctx` is { selected = node or nil, expanded = { ids of the nodes open as drawn } }
--   A menu item may have `editor = true`: after its `run` the keyboard goes to the editor (as `open` does).
--                 a view of its own in the side bar: a button in its header, the title, and these
--                 buttons at the header's right (before Collapse All)
--   keys = { { key = "/", desc = "Search", run = function(node) end } }
--                 extra single keys while the view has the keyboard (node: the selected one, or nil);
--                 the tree's own keys (j k h l, arrows, Enter, Space, Esc...) can't be taken
--
-- `:EshviTree {show|focus|hide|toggle} {view}`, for mappings.
--
-- A node: { id (stable across refreshes, unique in the view), label, hint, kind
-- (an LSP SymbolKind name, drawn as the completion menu's badge), icon + hl (a
-- glyph and a highlight group for its color), tooltip, line, end_line,
-- expandable, group, children } and any fields of the source's own, which come back
-- in `children`, `accept` and `menu`. `message` replaces the rows ("No
-- symbols found"); `expand` is how many levels are open at first (1); `buf`
-- is the buffer the tree is of, for the app to match a Markdown preview with.
--
-- A node may give `fg` (a number, 0xRRGGBB) instead of `hl` for its color.
--
-- A node may give `open = true`: it starts open the first time the app receives it (in a set,
-- an update or a lazy answer, at any depth). A later update never opens it again once the user
-- closed it; a new `set` of the roots starts over. The selection does not move. For a source
-- that hands over a chain of nodes with their children inline and wants them shown.
--
-- A node may give `folder = true`: a row that only holds others. A click, Enter and Space open or close it,
-- and the source's `accept` isn't called.
--
-- To the app: `gui_tree_set { view, gen, parent, nodes, message, expand, buf, menu, merge }`,
-- `gui_tree_select { view, id, path }`, `gui_tree_menu { view, id, items }`,
-- `gui_tree_register { view, side, keys }`, `gui_tree_show { view, action, focus }`,
-- `gui_tree_unregister { view }`. Back, as calls: `Eshvi.tree.expand(view, id,
-- gen)`, `Eshvi.tree.accept(view, id, how)`, `Eshvi.tree.menu(view, id)`,
-- `Eshvi.tree.run(view, id, index)`, `Eshvi.tree.button(view, index)`,
-- `Eshvi.tree.key(view, id, key)` and `Eshvi.tree.filter(view, group, shown)`.
local chan = ...

_G.Eshvi = _G.Eshvi or {}
local T = {}
Eshvi.tree = T

-- By view: { source, gen, nodes = id -> node, parents = id -> parent id, roots, menu, off = filter ids hidden -> true }
local views = {}

local function foreground(group)
  if not group or group == '' then
    return nil
  end
  local ok, hl = pcall(vim.api.nvim_get_hl, 0, { name = group, link = false })
  return ok and hl.fg or nil
end

-- What the app draws of a node (the rest stays here, by id).
local function wire(node)
  local out = {
    id = node.id,
    label = node.label,
    hint = node.hint,
    kind = node.kind,
    icon = node.icon,
    hl = node.hl,
    fg = node.fg or foreground(node.hl),
    tooltip = node.tooltip,
    line = node.line,
    end_line = node.end_line,
    expandable = node.expandable or nil,
    group = node.group,
    open = node.open or nil,
    folder = node.folder or nil,
  }
  if node.children and #node.children > 0 then
    out.children = vim.tbl_map(wire, node.children)
  end
  return out
end

-- Keep `nodes` (and their children) by id, so the app's calls find them.
local function index(v, nodes, parent)
  for _, node in ipairs(nodes) do
    v.nodes[node.id] = node
    v.parents[node.id] = parent
    if node.children then
      index(v, node.children, node.id)
    end
  end
end

-- The filters switched off, by view ({ slang = { "port", ... } }), kept between runs, and under `_known` the
-- filters each view had when it was saved: a filter added since then starts with its own default.
local function filters_file()
  return vim.fn.stdpath('state') .. '/eshvi_tree_filters.json'
end

local function saved_filters()
  local ok, lines = pcall(vim.fn.readfile, filters_file())
  if not ok then
    return {}
  end
  local decoded, saved = pcall(vim.json.decode, table.concat(lines, '\n'))
  return decoded and type(saved) == 'table' and saved or {}
end

-- Keys the tree uses itself (`tree_view.rs`): a source can't take them. (`f` opens the Filter by Name field.)
local own_keys = { j = true, k = true, h = true, l = true, f = true, [' '] = true, ['<Space>'] = true, ['<CR>'] = true, ['<Esc>'] = true }

function T.register(view, source)
  source = source or {}
  local v = { source = source, gen = 0, nodes = {}, parents = {}, roots = {}, keys = {}, buttons = {}, off = {} }
  views[view] = v
  local side
  if source.side then
    side = { title = source.side.title, icon = source.side.icon, tooltip = source.side.tooltip, buttons = {}, filters = {} }
    for i, b in ipairs(source.side.buttons or {}) do
      v.buttons[i] = b.run
      side.buttons[i] = { icon = b.icon, tooltip = b.tooltip }
    end
    local saved = source.side.filters and saved_filters() or {}
    local off = type(saved[view]) == 'table' and saved[view] or nil
    -- (A file saved before `_known` existed: a filter that isn't switched off in it may never have been seen.)
    local known = type(saved._known) == 'table' and type(saved._known[view]) == 'table' and saved._known[view] or {}
    v.filter_ids = {}
    for i, f in ipairs(source.side.filters or {}) do
      local hidden = f.hidden or false
      if off and vim.tbl_contains(off, f.id) then
        hidden = true
      elseif off and vim.tbl_contains(known, f.id) then
        hidden = false
      end
      side.filters[i] = { id = f.id, label = f.label, hidden = hidden }
      v.off[f.id] = hidden or nil
      table.insert(v.filter_ids, f.id)
    end
  end
  local keys = {}
  for _, k in ipairs(source.keys or {}) do
    if type(k.key) == 'string' and not own_keys[k.key] and not v.keys[k.key] then
      v.keys[k.key] = k.run
      table.insert(keys, k.key)
    end
  end
  if side or #keys > 0 then
    vim.rpcnotify(chan, 'gui_tree_register', { view = view, side = side, keys = keys })
  end
end

-- The names of the views registered, in no order.
function T.names()
  return vim.tbl_keys(views)
end

function T.unregister(view)
  if views[view] then
    views[view] = nil
    vim.rpcnotify(chan, 'gui_tree_unregister', { view = view })
  end
end

local function send_set(v, view, opts)
  vim.rpcnotify(chan, 'gui_tree_set', {
    view = view,
    gen = v.gen,
    nodes = vim.tbl_map(wire, v.roots),
    message = opts.message,
    expand = opts.expand,
    buf = opts.buf,
    menu = v.source.menu ~= nil or nil,
  })
end

function T.set(view, nodes, opts)
  local v = views[view]
  if not v then
    return
  end
  opts = opts or {}
  v.gen = v.gen + 1
  v.nodes, v.parents, v.roots, v.opts = {}, {}, nodes or {}, opts
  index(v, v.roots, nil)
  send_set(v, view, opts)
end

-- `nodes` into `old` by id: a node that is in both keeps the children it had
-- unless the new one brings some (recursively); the ones missing from `nodes` go.
local function merge(old, nodes)
  local by = {}
  for _, n in ipairs(old or {}) do
    by[n.id] = n
  end
  for _, n in ipairs(nodes) do
    local o = by[n.id]
    if o then
      if n.children and #n.children > 0 then
        n.children = merge(o.children, n.children)
      else
        n.children = o.children
      end
    end
  end
  return nodes
end

local function reindex(v)
  v.nodes, v.parents = {}, {}
  index(v, v.roots, nil)
end

function T.update(view, parent_id, nodes)
  local v = views[view]
  if not v then
    return
  end
  nodes = nodes or {}
  -- (What the app is told is what was given: it merges the same way.)
  local sent = vim.tbl_map(wire, nodes)
  if parent_id == nil then
    v.roots = merge(v.roots, nodes)
  else
    local node = v.nodes[parent_id]
    if not node then
      return
    end
    node.children = merge(node.children, nodes)
  end
  reindex(v)
  vim.rpcnotify(chan, 'gui_tree_set', { view = view, gen = v.gen, parent = parent_id, nodes = sent, merge = true, menu = v.source.menu ~= nil or nil })
end

-- Show the view in the side bar, or `hide` it (back to the Explorer). The view
-- needs `side` in its source.
function T.show(view, opts)
  if views[view] then
    vim.rpcnotify(chan, 'gui_tree_show', { view = view, action = 'show', focus = (opts and opts.focus) or false })
  end
end

function T.hide(view)
  if views[view] then
    vim.rpcnotify(chan, 'gui_tree_show', { view = view, action = 'hide', focus = false })
  end
end

-- `:EshviTree action view` (`focus` twice gives the keyboard back).
function T.command(action, view)
  if views[view] then
    vim.rpcnotify(chan, 'gui_tree_show', { view = view, action = action, focus = false })
  end
end

-- A header button of the view was clicked (its place, from 1).
function T.button(view, idx, selected, expanded)
  local v = views[view]
  local run = v and v.buttons[idx]
  if run then
    local ok, err = pcall(run, { selected = selected and v.nodes[selected] or nil, expanded = expanded or {} })
    if not ok then
      vim.notify('Eshvi.tree: button ' .. tostring(idx) .. ': ' .. tostring(err), vim.log.levels.ERROR)
    end
  end
end

-- A toggle of the view's Filter menu changed: the app has hidden or shown the rows,
-- this remembers it.
function T.filter(view, group, shown)
  local v = views[view]
  if not v then
    return
  end
  v.off[group] = (not shown) or nil
  local saved = saved_filters()
  local list = vim.tbl_keys(v.off)
  table.sort(list)
  saved[view] = list
  saved._known = type(saved._known) == 'table' and saved._known or {}
  saved._known[view] = v.filter_ids or {}
  local dir = vim.fn.stdpath('state')
  local ok, err = pcall(function()
    vim.fn.mkdir(dir, 'p')
    vim.fn.writefile({ vim.json.encode(saved) }, filters_file())
  end)
  if not ok then
    vim.notify('Eshvi.tree: could not save the filters: ' .. tostring(err), vim.log.levels.WARN)
  end
end

-- A key of the source's `keys` was pressed in the view; `id` is the selected node or nil.
function T.key(view, id, key)
  local v = views[view]
  local run = v and v.keys[key]
  if run then
    local ok, err = pcall(run, id and v.nodes[id] or nil)
    if not ok then
      vim.notify('Eshvi.tree: key ' .. tostring(key) .. ': ' .. tostring(err), vim.log.levels.ERROR)
    end
  end
end

function T.select(view, id)
  local v = views[view]
  if not v or not v.nodes[id] then
    return
  end
  local path, at = {}, id
  while at do
    table.insert(path, 1, at)
    at = v.parents[at]
  end
  vim.rpcnotify(chan, 'gui_tree_select', { view = view, id = id, path = path })
end

-- The app asks for the children of `id` (it was expanded, with none yet).
function T.expand(view, id, gen)
  local v = views[view]
  local node = v and v.nodes[id]
  if not node or not v.source.children then
    return
  end
  local answered = false
  local ok, err = pcall(v.source.children, node, function(children)
    if answered then
      return
    end
    answered = true
    -- (A refresh in between made the app's request stale: it says so by the gen.)
    if v.gen == gen and views[view] == v then
      node.children = children
      index(v, children, id)
    end
    vim.rpcnotify(chan, 'gui_tree_set', { view = view, gen = gen, parent = id, nodes = vim.tbl_map(wire, children or {}) })
  end)
  if not ok then
    vim.notify('Eshvi.tree: children of ' .. tostring(id) .. ': ' .. tostring(err), vim.log.levels.ERROR)
  end
end

function T.accept(view, id, how)
  local v = views[view]
  local node = v and v.nodes[id]
  if not node or not v.source.accept then
    return
  end
  local ok, err = pcall(v.source.accept, node, how)
  if not ok then
    vim.notify('Eshvi.tree: accept ' .. tostring(id) .. ': ' .. tostring(err), vim.log.levels.ERROR)
  end
end

-- The app asked for the right-click menu of `id`: its labels, kept to run one.
function T.menu(view, id)
  local v = views[view]
  local node = v and v.nodes[id]
  if not node or not v.source.menu then
    return
  end
  local ok, items = pcall(v.source.menu, node)
  if not ok then
    vim.notify('Eshvi.tree: menu ' .. tostring(id) .. ': ' .. tostring(items), vim.log.levels.ERROR)
    items = nil
  end
  v.menu = { id = id, items = items or {} }
  local labels = {}
  for _, item in ipairs(v.menu.items) do
    table.insert(labels, item.label)
  end
  vim.rpcnotify(chan, 'gui_tree_menu', { view = view, id = id, items = labels })
end

function T.run(view, id, idx)
  local v = views[view]
  local menu = v and v.menu
  local node = v and v.nodes[id]
  local item = menu and menu.id == id and menu.items[idx]
  if not (item and node and item.run) then
    return
  end
  v.menu = nil
  local ok, err = pcall(item.run, node)
  if not ok then
    vim.notify('Eshvi.tree: ' .. tostring(item.label) .. ': ' .. tostring(err), vim.log.levels.ERROR)
  end
  if item.editor then
    vim.rpcnotify(chan, 'gui_tree_show', { view = view, action = 'editor', focus = false })
  end
end

-- A new colorscheme: the colors of the nodes' groups are sent again.
vim.api.nvim_create_autocmd('ColorScheme', {
  group = vim.api.nvim_create_augroup('eshvi_tree', { clear = true }),
  callback = function()
    for view, v in pairs(views) do
      if v.opts then
        send_set(v, view, v.opts)
      end
    end
  end,
})
