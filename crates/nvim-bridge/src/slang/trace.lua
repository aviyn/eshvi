-- Signal trace: who drives a signal, or reads it, as a view of its own in Eshvi's side bar
-- (`Eshvi.tree`, NOTES.md "Outline and the native tree view"), built the way
-- `hierarchy.lua` is and sharing its helpers. Part of the app (`eshvi.slang.trace`, set up by `eshvi.slang`). The root is the traced signal, its children are
-- its drivers (or loads), and so on, one step at a time, across module boundaries. Outside Eshvi
-- (no `Eshvi.tree`) it does nothing.
--
--   M.register() / M.unregister()           the view (`eshvi.slang` calls them as a server attaches and goes)
--   M.trace(direction)                      the signal under the cursor; direction: "drivers" or "loads"
--   M.trace_path(path, direction, opts)     a signal by its hierarchical path (opts: show, wanted, location, signal)
--   M.endpoints(direction)                  a picker of where the signal under the cursor really ends up, past the wiring
--   M.endpoints_path(path, direction)       the same from a hierarchical path
--   M.switch()                              the same signal, the other direction
--   M.refresh(ctx)                          the same trace asked again; what is open stays open
local hierarchy = require('eshvi.slang.hierarchy')

local M = {}

local VIEW = 'slang_trace'
local MESSAGE = 'Press <leader>vt on a signal'

-- Rows of an answer whose own next step is asked for at once (so a row with nothing further has no
-- chevron and shows its count).
local PEEK_ROWS = 64
-- How deep a chain of single steps is followed by itself.
local FOLLOW_DEPTH = 24
-- An answer of more rows than this, in 2 instances or more, is grouped under the instances.
local GROUP_OVER = 8
-- Server questions one build may ask (peeks and chains), against a design that fans out.
local BUDGET = 600

local COMMANDS = { drivers = 'slang.getDriversWithLocation', loads = 'slang.getLoadsWithLocation' }
local TITLES = { drivers = 'Drivers', loads = 'Loads' }
-- A signal's drivers, and where it is assigned a value no signal feeds: no row stands for those.
local DRIVERS_WITH_CONSTANTS = 'slang.getDriversWithConstants'
-- Server questions one search for endpoints may ask.
local ENDPOINT_QUERIES = 3000
local ENDPOINT_TITLES = { drivers = 'Driver Endpoints', loads = 'Load Endpoints' }
-- svtrace (crates/svtrace), a second language server that answers the three trace commands from slang's
-- elaborated AST; the trace asks it instead of slang-server when it is attached.
local SVTRACE = hierarchy.SVTRACE

-- Whether svtrace has no design (its build was set aside by `svtrace.useBuild`, or never built): then the
-- trace asks slang-server, as the hierarchy does. Looked at (`probe`) when a trace starts.
local svtrace_without_design = false

-- An svtrace client and a buffer it is attached to, as `find_client` finds slang-server's. None while
-- svtrace has no design.
local function trace_client()
  if svtrace_without_design then
    return nil
  end
  return hierarchy.find_client(SVTRACE)
end

-- Asks svtrace whether it has a design (`svtrace.status` doesn't wait for a build; a build still running
-- counts as a design, its questions wait for it), then `done()`.
local function probe(done)
  local client = hierarchy.find_client(SVTRACE)
  local provider = client and client.server_capabilities and client.server_capabilities.executeCommandProvider
  if not (provider and vim.tbl_contains(provider.commands or {}, 'svtrace.status')) then
    svtrace_without_design = false
    done()
    return
  end
  hierarchy.execute('svtrace.status', {}, function(err, status)
    svtrace_without_design = not err and type(status) == 'table' and status.design == false and status.state ~= 'building'
    done()
  end, SVTRACE)
end

-- `workspace/executeCommand` for the trace commands: svtrace when it is attached, else slang-server.
local function trace_execute(command, arguments, done)
  hierarchy.execute(command, arguments, done, trace_client() and SVTRACE or nil)
end

-- The client that answers the trace commands (its encoding is the one the answers' columns are in).
local function answering_client()
  return trace_client() or hierarchy.find_client()
end

-- Whether the trace asks svtrace for whole steps and walks (`svtrace.step`, `svtrace.endpoints`): it is attached
-- and says it has them (an older svtrace has only the three commands, which the phase 1 path asks).
local STEP = 'svtrace.step'
local ENDPOINTS = 'svtrace.endpoints'
-- (Elaborates again: what Refresh asks for first.)
local RELOAD = 'svtrace.reload'

local function step_mode()
  local client = trace_client()
  local provider = client and client.server_capabilities and client.server_capabilities.executeCommandProvider
  return provider ~= nil and vim.tbl_contains(provider.commands or {}, STEP)
end

-- Whether svtrace merges the instances of a module (`paths` in `svtrace.step` and `svtrace.endpoints`, which
-- it says in `experimental.svtrace.paths`): a trace from the cursor then starts on all of them, with no picker.
local function paths_mode()
  if not step_mode() then
    return false
  end
  local capabilities = trace_client().server_capabilities
  local experimental = capabilities and capabilities.experimental
  return type(experimental) == 'table' and type(experimental.svtrace) == 'table' and experimental.svtrace.paths == true
end

-- The arguments that name where a step or a walk starts: `paths` (and `prefer`) for several, else `path`.
local function starts_of(paths, prefer)
  if #paths > 1 and paths_mode() then
    return { paths = paths, prefer = prefer }
  end
  return { path = paths[1] }
end

-- Whether svtrace says it finds the selected paths under the cursor (`svtrace.atCursor`).
local AT_CURSOR = 'svtrace.atCursor'

local function at_cursor_mode()
  local client = trace_client()
  local provider = client and client.server_capabilities and client.server_capabilities.executeCommandProvider
  return provider ~= nil and vim.tbl_contains(provider.commands or {}, AT_CURSOR)
end

local function loads_safe()
  return vim.g.slang_loads_safe == true or trace_client() ~= nil
end

-- Which directions may ask ahead. Not loads: slang-server 0.3.1 crashes answering the loads of a signal that
-- is connected to an instance with an unconnected port (`LoadsTracer` doesn't check for no expression, which
-- `DriversTracer` does), and asking ahead would hit one by itself; a whole session's server goes with it.
-- `vim.g.slang_loads_safe` says the server has the fix: loads then get counts and chains too.
-- svtrace answers loads without that crash, so with it attached they count as safe too.
local function asks_ahead(direction)
  return direction == 'drivers' or loads_safe()
end

local registered = false
-- A newer trace, a refresh or a stop makes older answers stale.
local generation = 0
-- What is shown: { path, direction }.
local current = nil

local function available()
  return hierarchy.available()
end

-- ---- Paths ----

-- A package enum's member (`pkg::enum_e.MEMBER`) is a constant, not a signal. A module-local enum
-- can't be told from a signal by the path alone.
local function is_constant(path)
  return path:match('^[%w_$]+::[%w_$]+%.[%w_$]+$') ~= nil
end

-- Whether an entry is a constant: svtrace says (its graph knows a module-local enum's member), else the path does.
local function entry_constant(entry)
  if type(entry.constant) == 'boolean' then
    return entry.constant
  end
  return is_constant(entry.path)
end

-- Where the last part of a path starts and the separator before it ends: the last `.` or `::` outside
-- brackets (`top.x[7:0]` has its last part at `x[7:0]`, `top.u[0].d` at `d`). nil for a path of one part.
local function split_at(path)
  local depth, from, stop = 0, nil, nil
  local i = 1
  while i <= #path do
    local c = path:sub(i, i)
    if c == '[' then
      depth = depth + 1
    elseif c == ']' then
      depth = math.max(depth - 1, 0)
    elseif depth == 0 then
      if c == '.' then
        stop, from = i - 1, i + 1
      elseif c == ':' and path:sub(i + 1, i + 1) == ':' then
        stop, from = i - 1, i + 2
        i = i + 1
      end
    end
    i = i + 1
  end
  return stop, from
end

-- The last part of a (selected) path: `mie` of `top.u.mstatus_q.mie`, `x[7:0]` of `top.x[7:0]`.
local function last_part(path)
  local _, from = split_at(path)
  return from and path:sub(from) or path
end

-- The instance a path is in: up to its last `.` (a package's `pkg::x` is in `pkg`). Of a signal's plain path;
-- a selected path's instance is its signal's.
local function instance_of(path)
  local stop = split_at(path)
  return stop and path:sub(1, stop) or ''
end

-- The plain path of a selected path nothing else says (a trace's root): without trailing selects.
local function plain_of(path)
  local plain = path
  while plain:match('%b[]$') do
    plain = plain:gsub('%b[]$', '')
  end
  return plain
end

-- The plain signal path of an entry or row: `signal` when svtrace gives one, else the path.
local function signal_of(entry)
  if type(entry.signal) == 'string' and entry.signal ~= '' then
    return entry.signal
  end
  return entry.path
end

-- An instance path without the top module (the top itself keeps its name, or its rows would say nothing).
local function inside_top(instance)
  return instance:match('^[^%.]*%.(.*)$') or instance
end

-- ---- The server ----

local function query(direction, path, done)
  trace_execute(COMMANDS[direction], { path }, done)
end

-- The entries of an answer, once each, by path.
local function normalize(result)
  local seen, out = {}, {}
  if type(result) ~= 'table' then
    return out
  end
  for _, item in ipairs(result) do
    if type(item) == 'table' and type(item.path) == 'string' and not seen[item.path] then
      seen[item.path] = true
      table.insert(out, item)
    end
  end
  table.sort(out, function(a, b)
    return a.path < b.path
  end)
  return out
end

-- Run `fn(item, finish)` for each item, all asked at once; `done()` when every one has finished.
local function each(items, fn, done)
  local left = #items
  if left == 0 then
    done()
    return
  end
  for _, item in ipairs(items) do
    fn(item, function()
      left = left - 1
      if left == 0 then
        done()
      end
    end)
  end
end

-- ---- Rows ----

local function decorate(node, kind)
  local dec = hierarchy.decoration(kind)
  node.icon = dec and dec.icon or '?'
  node.hl = dec and dec.hl
  node.fg = hierarchy.color_of(dec and dec.hl)
end

-- Whether a row only decides if its connections happen (an `if` or `case`), not what they carry.
local function control_only(sites)
  for _, site in ipairs(sites) do
    if site.kind ~= 'control' then
      return false
    end
  end
  return #sites > 0
end

-- Whether a row is only read as a clock (an edge in an event control): not a step of the signal's logic.
local function clock_only(sites)
  for _, site in ipairs(sites) do
    if site.kind ~= 'clock' then
      return false
    end
  end
  return #sites > 0
end

local function new_row(entry, parent, level, direction)
  local path = entry.path
  local location = entry.location or {}
  local start = location.range and location.range.start or {}
  local constant = entry_constant(entry)
  -- (Absent from a server without the sites: the row then has only its declaration.)
  local sites = type(entry.sites) == 'table' and entry.sites or {}
  local ancestors = vim.list_slice(parent.ancestors)
  local node = {
    id = parent.id .. '>' .. path,
    path = path,
    signal = signal_of(entry),
    label = last_part(path),
    tooltip = path,
    uri = location.uri,
    line = start.line,
    col = start.character,
    sites = sites,
    group = constant and 'constant' or (direction == 'drivers' and clock_only(sites) and 'clock') or (control_only(sites) and 'control') or nil,
    constant = constant,
    direction = direction,
    instance = inside_top(instance_of(signal_of(entry))),
    paths = type(entry.paths) == 'table' and #entry.paths > 0 and entry.paths or { path },
    level = level,
    -- A row whose path is one of its own ancestors starts the chain over: it is not followed.
    loop = vim.tbl_contains(ancestors, path),
  }
  -- (svtrace says: it counts the chain above the row in the same answer too.)
  if type(entry.loop) == 'boolean' then
    node.loop = entry.loop
  end
  -- (All the paths of a merged row are above its children.)
  vim.list_extend(ancestors, node.paths)
  node.ancestors = ancestors
  -- A row for only some of the instances above it says which.
  local above = parent.paths or { parent.path }
  if type(entry.from) == 'table' and #entry.from > 0 and #entry.from < #above then
    local names = {}
    for _, from in ipairs(entry.from) do
      local name = inside_top(instance_of(from))
      if not vim.tbl_contains(names, name) then
        table.insert(names, name)
      end
    end
    -- (The instance's own name in the hint, which a narrow side bar would cut from a whole path; the whole
    -- names in the tooltip.)
    node.split = #entry.from == 1 and last_part(names[1]) or (#entry.from .. ' of ' .. #above)
    node.tooltip = path .. '\n' .. table.concat(names, ', ')
  elseif #node.paths > 1 then
    -- (A row for several instances names no one instance: its paths are in the tooltip.)
    node.tooltip = table.concat(node.paths, '\n')
  end
  -- (A constant is drawn as the hierarchy draws a parameter, which it is closest to.)
  decorate(node, constant and 'Param' or 'Logic')
  return node
end

-- Whether an entry (or a row) is a signal: a constant isn't one, and is hidden unless asked for; nor is a
-- driver that is only a clock, which a flop's chain doesn't count.
local function is_signal(entry, direction)
  if entry_constant(entry) then
    return false
  end
  return not (direction == 'drivers' and clock_only(type(entry.sites) == 'table' and entry.sites or {}))
end

-- How many of an answer's entries are signals.
local function signals(entries, direction)
  local count = 0
  for _, entry in ipairs(entries) do
    if is_signal(entry, direction) then
      count = count + 1
    end
  end
  return count
end

-- The hint and the chevron, once the row's own next step is known (or not: `next` nil).
local function describe(node)
  -- (The count or the mark first: a narrow side bar cuts the end. The whole path is in the tooltip.)
  local parts = {}
  if node.loop then
    table.insert(parts, 'loop')
  elseif node.count then
    -- (svtrace's: counted in its graph, with the chevron's `total`.)
    if node.count > 0 then
      table.insert(parts, tostring(node.count))
    end
  elseif node.next and signals(node.next, node.direction) > 0 then
    table.insert(parts, tostring(signals(node.next, node.direction)))
  end
  if node.split then
    table.insert(parts, node.split)
  end
  -- (Not twice: a row inside the instance it is for. Not for a row of several instances, whose first path's
  -- instance would stand for all of them.)
  if node.instance ~= '' and #(node.paths or {}) <= 1 and node.split ~= last_part(node.instance) then
    table.insert(parts, last_part(node.instance))
  end
  node.hint = #parts > 0 and table.concat(parts, ' · ') or nil
  local more
  if node.total then
    more = node.total > 0
  else
    more = node.next == nil or #node.next > 0
  end
  node.expandable = (not node.loop and not node.constant and more) or nil
end

-- Exactly one next step that isn't a constant.
local function single_step(entries, direction)
  return signals(entries, direction) == 1
end

-- Whether the signals among `rows` are in more than one instance. (Signals as `signals` counts them, and as
-- svtrace's "many" does: a driver that is only a clock is not one.)
local function spans_instances(rows, direction)
  local first
  for _, row in ipairs(rows) do
    if is_signal(row, direction) then
      local instance = instance_of(row.signal)
      first = first or instance
      if instance ~= first then
        return true
      end
    end
  end
  return false
end

-- The signals under instance rows (no `group`, children inline, closed), by instance path; constants
-- after them, in no instance, for the Filter menu to hide.
local function grouped(parent, rows, job)
  local by, names, constants = {}, {}, {}
  for _, row in ipairs(rows) do
    if row.constant then
      table.insert(constants, row)
    else
      local instance = instance_of(row.signal)
      if not by[instance] then
        by[instance] = {}
        table.insert(names, instance)
      end
      table.insert(by[instance], row)
    end
  end
  table.sort(names)
  local out = {}
  for _, instance in ipairs(names) do
    local group = {
      id = parent.id .. '>@' .. instance,
      label = last_part(instance),
      hint = tostring(#by[instance]),
      tooltip = instance,
      is_group = true,
      folder = true,
      children = by[instance],
      open = job.wanted and job.wanted[parent.id .. '>@' .. instance] or nil,
    }
    -- (What the instance is under, for telling two of the same name apart.)
    local above = instance_of(inside_top(instance))
    if above ~= '' then
      group.hint = group.hint .. ' · ' .. above
    end
    decorate(group, 'Instance')
    table.insert(out, group)
  end
  return vim.list_extend(out, constants)
end

local build_rows

-- Whether a row's chain goes on by itself (or, in a refresh, because it was open).
local function follows(row, many, job)
  if row.loop or row.constant then
    return false
  end
  if job.wanted then
    return job.wanted[row.id] == true
  end
  return not many and row.next ~= nil and row.level < FOLLOW_DEPTH and single_step(row.next, job.direction)
end

-- The rows for `entries` under `parent`: each one's own next step asked for (the first PEEK_ROWS), the
-- chains of single steps followed with their children inline and marked open. `done(rows)` once. A
-- stale job (a newer trace) never finishes.
function build_rows(parent, entries, level, job, done)
  local rows = {}
  for _, entry in ipairs(entries) do
    table.insert(rows, new_row(entry, parent, level, job.direction))
  end
  local many = signals(rows, job.direction) > GROUP_OVER and spans_instances(rows, job.direction)
  local peeked = {}
  for i, row in ipairs(rows) do
    if asks_ahead(job.direction) and i <= PEEK_ROWS and not row.loop and not row.constant then
      table.insert(peeked, row)
    end
  end
  each(peeked, function(row, finish)
    if job.budget <= 0 then
      finish()
      return
    end
    job.budget = job.budget - 1
    query(job.direction, row.path, function(err, result)
      if job.gen ~= generation then
        return
      end
      -- (An answer that fails leaves the row as one that may have something: asked again when opened.)
      if not err then
        row.next = normalize(result)
      end
      finish()
    end)
  end, function()
    local going = {}
    for _, row in ipairs(rows) do
      describe(row)
      if follows(row, many, job) then
        table.insert(going, row)
      end
    end
    each(going, function(row, finish)
      local function go()
        build_rows(row, row.next, level + 1, job, function(children)
          row.children = children
          row.open = true
          finish()
        end)
      end
      if row.next then
        go()
      else
        query(job.direction, row.path, function(err, result)
          if job.gen ~= generation then
            return
          end
          if err then
            finish()
            return
          end
          row.next = normalize(result)
          describe(row)
          go()
        end)
      end
    end, function()
      done(many and grouped(parent, rows, job) or rows)
    end)
  end)
end

-- ---- Rows from svtrace.step ----

-- `svtrace.step` for one signal: its entries (sorted, once each), with `count`, `total` and the chain that
-- goes on by itself as `next` when `follow`. `ancestors` is the chain above, for the loop mark.
local function step_query(direction, paths, prefer, ancestors, follow, done)
  local argument = starts_of(paths, prefer)
  argument.direction, argument.ancestors, argument.follow = direction, ancestors, follow
  trace_execute(STEP, { argument }, done)
end

local build_step_rows

-- The rows for `entries` under `parent`, from svtrace's answer: the chevron and the count are the entry's, a
-- chain that came with `next` is inline and open. In a refresh (`job.wanted`) the answer has no chains, and each
-- row that was open is asked for in turn, a level at a time. Grouping ("many") is still ours. `done(rows)` once.
function build_step_rows(parent, entries, level, job, done)
  local rows = {}
  for _, entry in ipairs(entries) do
    local row = new_row(entry, parent, level, job.direction)
    row.count, row.total = entry.count, entry.total
    row.next = type(entry.next) == 'table' and normalize(entry.next) or nil
    table.insert(rows, row)
  end
  local many = signals(rows, job.direction) > GROUP_OVER and spans_instances(rows, job.direction)
  local asked = {}
  if job.wanted then
    for _, row in ipairs(rows) do
      if job.wanted[row.id] and not row.loop and not row.constant then
        table.insert(asked, row)
      end
    end
  end
  each(asked, function(row, finish)
    step_query(job.direction, row.paths, job.prefer, row.ancestors, false, function(err, result)
      if job.gen ~= generation then
        return
      end
      if not err then
        row.next = normalize(result)
      end
      finish()
    end)
  end, function()
    local going = {}
    for _, row in ipairs(rows) do
      describe(row)
      if row.next and not row.loop and not row.constant and (not job.wanted or job.wanted[row.id]) then
        table.insert(going, row)
      end
    end
    each(going, function(row, finish)
      build_step_rows(row, row.next, level + 1, job, function(children)
        row.children = children
        row.open = true
        finish()
      end)
    end, function()
      done(many and grouped(parent, rows, job) or rows)
    end)
  end)
end

-- ---- Tracing ----

-- The trace of `path`: `opts.show = false` leaves the side bar as it is; `opts.wanted` (ids -> true) opens
-- only those rows (a refresh), else the chains of single steps open by themselves; `opts.location`
-- ({ uri, line, col }) is where the root is (the server's answer describes only its drivers and loads); `opts.signal`
-- is the plain path of the signal when `path` is a selected one (a row that becomes the root knows it); `opts.paths`
-- (with `path` the first) are the instances of a module that svtrace merges, `opts.prefer` the instance whose paths
-- come first.
function M.trace_path(path, direction, opts)
  if not (registered and available()) then
    return
  end
  opts = opts or {}
  direction = direction == 'loads' and 'loads' or 'drivers'
  generation = generation + 1
  local mine = generation
  local stepping = step_mode()
  -- (Several paths only where svtrace merges them; the first alone otherwise.)
  local paths = opts.paths and #opts.paths > 0 and paths_mode() and opts.paths or { path }
  local function ask(reply)
    if not stepping then
      query(direction, path, reply)
    else
      -- (A trace is one request, chains included; a refresh asks for the open rows afterwards.)
      step_query(direction, paths, opts.prefer, {}, not opts.wanted, reply)
    end
  end
  ask(function(err, result)
    if mine ~= generation or not registered then
      return
    end
    if err then
      hierarchy.fail(err)
      return
    end
    local entries = normalize(result)
    local where = opts.location or {}
    local root = {
      id = path, path = path, signal = opts.signal or plain_of(path), label = last_part(path), tooltip = path, level = 0, ancestors = vim.list_slice(paths), instance = '',
      paths = paths, uri = where.uri, line = where.line, col = where.col,
    }
    if #paths > 1 then
      root.tooltip = table.concat(paths, '\n')
    end
    decorate(root, 'Logic')
    local job = { gen = mine, direction = direction, budget = BUDGET, wanted = opts.wanted, prefer = opts.prefer }
    local build = stepping and build_step_rows or build_rows
    build(root, entries, 1, job, function(rows)
      if mine ~= generation or not registered then
        return
      end
      root.children = rows
      root.hint = TITLES[direction] .. ' · ' .. signals(entries, direction)
      if #paths > 1 then
        root.hint = root.hint .. ' · ' .. #paths .. ' instances'
      end
      root.open = (not opts.wanted or opts.wanted[root.id]) or nil
      current = { path = path, direction = direction, location = opts.location, step = stepping, signal = root.signal, paths = paths, prefer = opts.prefer }
      Eshvi.tree.set(VIEW, { root }, { expand = 1 })
      if opts.show ~= false then
        Eshvi.tree.select(VIEW, root.id)
        Eshvi.tree.show(VIEW, { focus = true })
      end
    end)
  end)
end

-- The signal under the cursor: `start(path, location, signal, merge)` with its one instance, or several ask (the one
-- under the active instance, if there is one, without asking); with svtrace merging instances they all start the
-- trace (`merge` = { paths, prefer }, the active instance's first). False when the view isn't there.
local function from_cursor(start)
  if not (registered and available()) then
    return false
  end
  local by_svtrace = at_cursor_mode()
  -- (svtrace alone is enough when it finds the paths at the cursor; otherwise slang-server is asked.)
  local client = by_svtrace and trace_client() or hierarchy.find_client()
  if not client then
    hierarchy.fail('slang-server: no slang-server LSP client attached')
    return true
  end
  local params = vim.lsp.util.make_position_params(0, client.offset_encoding)
  -- (The root's place is where the cursor was: its declaration isn't in the answer.)
  local function chosen(item, merge)
    start(item.path, { uri = params.textDocument.uri, line = params.position.line, col = params.position.character }, item.signal, merge)
  end
  local function ask(done)
    if by_svtrace then
      -- (The selected paths of the expression there, one per instance of its module.)
      trace_execute(AT_CURSOR, { { textDocument = params.textDocument, position = params.position } }, done)
    else
      hierarchy.execute('slang.getInstances', { params }, done)
    end
  end
  ask(function(err, result)
    if err then
      hierarchy.fail(err)
      return
    end
    -- (A string, or { path } / { hierPath }, as slang-server's; svtrace's { path, signal } says the plain signal too.)
    local paths = {}
    for _, item in ipairs(type(result) == 'table' and result or {}) do
      local path = type(item) == 'string' and item or (type(item) == 'table' and (item.path or item.hierPath)) or nil
      if type(path) == 'string' and path ~= '' then
        local signal = type(item) == 'table' and type(item.signal) == 'string' and item.signal ~= '' and item.signal or nil
        table.insert(paths, { path = path, signal = signal })
      end
    end
    if #paths == 0 then
      vim.notify('Not a signal in the current design', vim.log.levels.WARN)
    elseif #paths == 1 then
      chosen(paths[1])
    elseif by_svtrace and paths_mode() then
      -- (No picker, and the active instance doesn't narrow it: it only comes first.)
      local active = hierarchy.active_path()
      local first, rest = {}, {}
      for _, item in ipairs(paths) do
        table.insert(active and item.path:sub(1, #active + 1) == active .. '.' and first or rest, item)
      end
      local all = vim.list_extend(first, rest)
      local list = {}
      for _, item in ipairs(all) do
        table.insert(list, item.path)
      end
      chosen(all[1], { paths = list, prefer = active })
    else
      local active = hierarchy.active_path()
      for _, item in ipairs(paths) do
        if active and item.path:sub(1, #active + 1) == active .. '.' then
          chosen(item)
          return
        end
      end
      local items = {}
      for i, item in ipairs(paths) do
        table.insert(items, { text = item.path, index = i })
      end
      Eshvi.picker.pick({
        title = 'Trace which instance?',
        items = items,
        on_choice = function(item)
          if item then
            chosen(paths[item.index])
          end
        end,
      })
    end
  end)
  return true
end

function M.trace(direction)
  return from_cursor(function(path, location, signal, merge)
    M.trace_path(path, direction, { location = location, signal = signal, paths = merge and merge.paths, prefer = merge and merge.prefer })
  end)
end

-- ---- Endpoints ----

-- A newer search makes an older one's answers stale.
local endpoint_generation = 0

-- A signal right under the top module: with nothing further, it is where the design's edge is.
local function at_top(path)
  return path:match('^[^%.:]+%.[^%.:]+$') ~= nil
end

-- Walks out from `path` while a connection is only wiring (a port, a plain copy) and collects where it stops:
-- a line that reads the signal (loads) or assigns it from logic or a constant (drivers; a condition isn't that
-- assignment). `done(locations, stopped)`, each line once; `stopped` when the question limit ended it early.
local function collect_endpoints(path, direction, done)
  endpoint_generation = endpoint_generation + 1
  local mine = endpoint_generation
  local seen, lines, found = { [path] = true }, {}, {}
  local left = ENDPOINT_QUERIES
  local function add(location)
    local key = location.uri .. ':' .. location.range.start.line
    if not lines[key] then
      lines[key] = true
      table.insert(found, location)
    end
  end
  -- `reply(err, entries, constants)`. (Loads have no constants.)
  local function ask(path, reply)
    if direction == 'loads' then
      query(direction, path, function(err, result)
        reply(err, result, {})
      end)
      return
    end
    trace_execute(DRIVERS_WITH_CONSTANTS, { path }, function(err, result)
      local answer = type(result) == 'table' and result or {}
      reply(err, answer.drivers, answer.constants or {})
    end)
  end
  local function step(frontier)
    local following = {}
    each(frontier, function(item, finish)
      if left <= 0 then
        finish()
        return
      end
      left = left - 1
      ask(item.path, function(err, result, constants)
        if mine ~= endpoint_generation then
          return
        end
        if err then
          -- (The signal asked for: said. Further on: something that can't be followed, as a module-local enum's member.)
          if item.entry then
            finish()
          else
            hierarchy.fail(err)
          end
          return
        end
        local entries = normalize(result)
        for _, entry in ipairs(entries) do
          for _, site in ipairs(not is_constant(entry.path) and type(entry.sites) == 'table' and entry.sites or {}) do
            if site.kind == 'port' or site.kind == 'wire' then
              if not seen[entry.path] then
                seen[entry.path] = true
                table.insert(following, { path = entry.path, entry = entry })
              end
            elseif site.kind == 'data' or (site.kind == 'control' and direction == 'loads') then
              add(site.location)
            end
          end
        end
        for _, site in ipairs(constants) do
          add(site.location)
        end
        if #entries == 0 and #constants == 0 and item.entry and at_top(item.path) then
          add(item.entry.location)
        end
        finish()
      end)
    end, function()
      if #following > 0 and left > 0 then
        step(following)
      else
        done(found, #following > 0)
      end
    end)
  end
  step({ { path = path } })
end

-- (`opts.paths`, `opts.prefer`: the instances svtrace merges, as `trace_path` has them.)
function M.endpoints_path(path, direction, opts)
  opts = opts or {}
  if not (registered and available()) then
    return
  end
  -- (Needs the sites, and walking loads would crash the stock server: `asks_ahead`.)
  if not loads_safe() then
    vim.notify('Endpoints need the patched slang-server', vim.log.levels.WARN)
    return
  end
  direction = direction == 'loads' and 'loads' or 'drivers'
  local function walk(done)
    if not step_mode() then
      collect_endpoints(path, direction, done)
      return
    end
    -- svtrace walks the graph itself: one request, no cap.
    endpoint_generation = endpoint_generation + 1
    local mine = endpoint_generation
    local argument = starts_of(opts.paths and #opts.paths > 0 and opts.paths or { path }, opts.prefer)
    argument.direction = direction
    trace_execute(ENDPOINTS, { argument }, function(err, result)
      if mine ~= endpoint_generation then
        return
      end
      if err then
        hierarchy.fail(err)
        return
      end
      done(type(result) == 'table' and result or {}, false)
    end)
  end
  walk(function(found, stopped)
    local title = ENDPOINT_TITLES[direction] .. ' of ' .. last_part(path)
    if stopped then
      vim.notify(title .. ': too many connections, these are the ones found so far', vim.log.levels.WARN)
    end
    if #found == 0 then
      vim.notify('No ' .. title:lower(), vim.log.levels.INFO)
    elseif #found == 1 then
      hierarchy.goto_location(found[1])
    else
      local client = answering_client()
      hierarchy.to_editor_window()
      vim.cmd("normal! m'")
      Eshvi.picker.locations({ title = title, locations = found, encoding = client and client.offset_encoding })
    end
  end)
end

function M.endpoints(direction)
  return from_cursor(function(path, _, _, merge)
    M.endpoints_path(path, direction, { paths = merge and merge.paths, prefer = merge and merge.prefer })
  end)
end

function M.switch()
  if current then
    M.trace_path(current.path, current.direction == 'drivers' and 'loads' or 'drivers', {
      show = false, location = current.location, signal = current.signal, paths = current.paths, prefer = current.prefer,
    })
  end
end

-- (`ctx.expanded`: the ids open as drawn, from the app.)
function M.refresh(ctx)
  if not current then
    return
  end
  local wanted = {}
  for _, id in ipairs(ctx and ctx.expanded or {}) do
    wanted[id] = true
  end
  local shown = current
  local function again()
    if current == shown then
      M.trace_path(shown.path, shown.direction, {
        show = false, wanted = wanted, location = shown.location, signal = shown.signal, paths = shown.paths, prefer = shown.prefer,
      })
    end
  end
  -- svtrace elaborates the design again first (`svtrace.reload` answers when that is done), so a refresh
  -- shows what the files hold now, whoever changed them. Its answer is the build's status: when the build
  -- failed, svtrace keeps the last design and says so itself, and the trace is asked again all the same.
  local client = trace_client()
  local provider = client and client.server_capabilities and client.server_capabilities.executeCommandProvider
  if provider ~= nil and vim.tbl_contains(provider.commands or {}, RELOAD) then
    trace_execute(RELOAD, {}, again)
  else
    again()
  end
end

-- ---- The view ----

-- A row opened: its next step was asked for with the row, else it is now.
local function children_of(node, reply)
  local mine = generation
  local direction = current and current.direction or 'drivers'
  local job = { gen = mine, direction = direction, budget = BUDGET, prefer = current and current.prefer }
  local stepping = current and current.step
  local function build(entries)
    (stepping and build_step_rows or build_rows)(node, entries, (node.level or 0) + 1, job, function(rows)
      if mine == generation then
        reply(rows)
      end
    end)
  end
  if node.next then
    build(node.next)
    return
  end
  local function ask(done)
    if stepping then
      step_query(direction, node.paths or { node.path }, job.prefer, node.ancestors or { node.path }, true, done)
    else
      query(direction, node.path, done)
    end
  end
  ask(function(err, result)
    if mine ~= generation then
      return
    end
    if err then
      hierarchy.fail(err)
      reply({})
      return
    end
    node.next = normalize(result)
    build(node.next)
  end)
end

local function goto_declaration(node)
  local start = node.line and { line = node.line, character = node.col or 0 } or nil
  hierarchy.goto_location({ uri = node.uri, range = start and { start = start } or nil })
end

-- Where the row connects to the row above it: one place is a jump, several ask. (Space keeps the
-- keyboard in the view, so it goes to the first.) A root, or a server without sites: its own place.
local function accept(node, how)
  if node.is_group then
    return
  end
  local sites = node.sites or {}
  if #sites == 0 then
    if node.uri then
      goto_declaration(node)
    end
    return
  end
  if #sites > 1 and how == 'open' then
    local locations = {}
    for i, site in ipairs(sites) do
      locations[i] = site.location
    end
    local client = answering_client()
    hierarchy.to_editor_window()
    vim.cmd("normal! m'")
    if Eshvi.picker.locations({
      title = 'Connections of ' .. node.label,
      locations = locations,
      encoding = client and client.offset_encoding,
    }) then
      return
    end
  end
  hierarchy.goto_location(sites[1].location)
end

-- Where a row is, for the root it becomes.
local function place(node)
  return { uri = node.uri, line = node.line, col = node.col }
end

local function menu(node)
  if node.is_group then
    return {}
  end
  local items = {
    {
      label = 'Copy Path',
      run = function(n)
        hierarchy.yank(n.path)
      end,
    },
  }
  -- (A selected path, `top.x[7:0]`: its signal's plain path is a second thing to copy.)
  if node.signal and node.signal ~= node.path then
    table.insert(items, {
      label = 'Copy Signal Path',
      run = function(n)
        hierarchy.yank(n.signal)
      end,
    })
  end
  -- (Without sites, accepting already goes there.)
  if node.uri and node.sites and #node.sites > 0 then
    table.insert(items, { label = 'Go to Declaration', run = goto_declaration, editor = true })
  end
  if not node.constant then
    table.insert(items, {
      label = 'Trace Drivers',
      run = function(n)
        M.trace_path(n.path, 'drivers', { location = place(n), signal = n.signal })
      end,
    })
    table.insert(items, {
      label = 'Trace Loads',
      run = function(n)
        M.trace_path(n.path, 'loads', { location = place(n), signal = n.signal })
      end,
    })
    table.insert(items, {
      label = 'Endpoints…',
      editor = true,
      run = function(n)
        M.endpoints_path(n.path, current and current.direction, { paths = n.paths, prefer = current and current.prefer })
      end,
    })
    table.insert(items, {
      label = 'Show in Hierarchy',
      run = function(n)
        hierarchy.reveal(n.signal or n.path, { focus = true })
      end,
    })
  end
  return items
end

function M.register()
  if registered or not available() then
    return
  end
  registered = true
  current = nil
  Eshvi.tree.register(VIEW, {
    side = {
      title = 'TRACE',
      icon = 'references',
      tooltip = 'Signal Trace',
      buttons = {
        {
          icon = 'arrow-swap',
          tooltip = 'Switch Drivers / Loads',
          run = function()
            M.switch()
          end,
        },
        {
          icon = 'refresh',
          tooltip = 'Refresh',
          run = function(ctx)
            M.refresh(ctx)
          end,
        },
      },
      filters = {
        { id = 'constant', label = 'Constants', hidden = true },
        { id = 'control', label = 'Control Signals' },
        { id = 'clock', label = 'Clock Signals', hidden = true },
      },
    },
    children = children_of,
    accept = accept,
    menu = menu,
    keys = {
      {
        key = 's',
        desc = 'Switch drivers / loads',
        run = function()
          M.switch()
        end,
      },
      {
        key = 'y',
        desc = 'Copy path',
        run = function(node)
          if node and node.path then
            hierarchy.yank(node.path)
          end
        end,
      },
    },
  })
  Eshvi.tree.set(VIEW, {}, { message = MESSAGE })
end

function M.unregister()
  if not registered then
    return
  end
  registered = false
  current = nil
  generation = generation + 1
  Eshvi.tree.unregister(VIEW)
end

-- Every way into a trace first looks at whether svtrace has a design (the entry points return what they
-- did, or true when they will).
for _, name in ipairs({ 'trace', 'endpoints', 'trace_path', 'endpoints_path', 'refresh' }) do
  local inner = M[name]
  M[name] = function(...)
    if not (registered and available()) then
      return inner(...)
    end
    local args = { n = select('#', ...), ... }
    probe(function()
      inner(unpack(args, 1, args.n))
    end)
    return true
  end
end

M.registered = function()
  return registered
end

return M
