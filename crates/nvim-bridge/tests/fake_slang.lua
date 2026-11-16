-- A fake slang-server for the hierarchy view's tests, on top of `fake_picker_lsp.lua`
-- (load that first). `fake_slang(opts)` starts it for the current buffer: it says it is
-- `slang-server`, advertises the commands, and answers them from a small fixed design:
--
--   top (Instance of top_mod)
--     clk  (Port, input logic)    q (Port, output logic)    io (Port, inout logic)
--     WIDTH (Param int = 8)
--     u (InstanceArray of leaf_mod)
--       [0] (Instance of leaf_mod)   d (Port input), r (Logic)
--       [1] (Instance of leaf_mod)
--   pkg (Package)
--     x (Param int = 3)
--
-- Everything it is asked is in `FAKE_CALLS` (the fake LSP keeps it); `SLANG.fail` makes
-- the commands answer with an error message; `SLANG.delay` (ms) delays every answer;
-- `SLANG.scopes` is the design, by path, for a test to change. `slang.showHierLocation`
-- opens the node's place in the current window, as the real server's `window/showDocument`.
--
-- The signal trace's commands answer from `SLANG.drivers` and `SLANG.loads` (path -> the paths one
-- step away; a path that is not a key is the real server's error), with the place `SLANG.lines[path]`
-- (0-based, 12 if not given), and `slang.getInstances` from `SLANG.instances` (cursor line, 0-based ->
-- the paths of the symbol there). The defaults, each a test starts from:
--
--   drivers: q <- a <- b <- c <- (x, y);  y <- (z, pkg::e.K);  z <- y (a loop);  x <- nothing
--            big9 <- 9 signals, in 2 instances;  big8 <- 8, in 2;  one9 <- 9, in 1
--            mix9 <- big9's and a constant;  onek <- one9's and a constant
--            u[0].d <- clk;  u[1].d <- q
--   loads:   a -> q;  q -> (out0, out1);  out0, out1 -> nothing
--   instances: line 2 -> q;  line 3 -> u[0].d, u[1].d;  line 4 -> big9
_G.SLANG = _G.SLANG or { fail = nil, delay = nil }

local function signals(prefix, count)
  local out = {}
  for i = 1, count do
    table.insert(out, string.format('%s%02d', prefix, i))
  end
  return out
end

local function default_graph()
  local big9 = {}
  for i = 1, 9 do
    table.insert(big9, string.format('top.m%d.s%d', i % 2, i))
  end
  local big8 = {}
  for i = 1, 8 do
    table.insert(big8, string.format('top.m%d.s%d', i % 2, i))
  end
  local drivers = {
    ['top.q'] = { 'top.a' },
    ['top.a'] = { 'top.b' },
    ['top.b'] = { 'top.c' },
    ['top.c'] = { 'top.x', 'top.y' },
    ['top.x'] = {},
    ['top.y'] = { 'top.z', 'pkg::e.K' },
    ['top.z'] = { 'top.y' },
    ['pkg::e.K'] = {},
    ['top.big9'] = big9,
    ['top.mix9'] = vim.list_extend(vim.deepcopy(big9), { 'pkg::e.K' }),
    ['top.big8'] = big8,
    ['top.one9'] = signals('top.m0.t', 9),
    ['top.onek'] = vim.list_extend(signals('top.m0.t', 9), { 'pkg::e.K' }),
    ['top.u[0].d'] = { 'top.clk' },
    ['top.u[1].d'] = { 'top.q' },
    ['top.clk'] = {},
  }
  for _, path in ipairs(big9) do
    drivers[path] = {}
  end
  for _, path in ipairs(big8) do
    drivers[path] = {}
  end
  for _, path in ipairs(drivers['top.one9']) do
    drivers[path] = {}
  end
  local loads = {
    ['top.a'] = { 'top.q' },
    ['top.q'] = { 'top.out0', 'top.out1' },
    ['top.out0'] = {},
    ['top.out1'] = {},
  }
  local instances = {
    [2] = { 'top.q' },
    [3] = { 'top.u[0].d', 'top.u[1].d' },
    [4] = { 'top.big9' },
  }
  return drivers, loads, instances
end

SLANG.drivers, SLANG.loads, SLANG.instances = default_graph()
SLANG.lines = { ['top.q'] = 2, ['top.a'] = 12, ['top.b'] = 13, ['top.c'] = 14 }

local function loc(file, line)
  return { uri = vim.uri_from_fname(file), range = { start = { line = line, character = 4 }, ['end'] = { line = line, character = 9 } } }
end

local function design(file)
  local top_kids = {
    { kind = 'Port', instName = 'clk', type = 'input logic', instLoc = loc(file, 1) },
    { kind = 'Port', instName = 'q', type = 'output logic', instLoc = loc(file, 2) },
    { kind = 'Port', instName = 'io', type = 'inout logic', instLoc = loc(file, 3) },
    { kind = 'Param', instName = 'WIDTH', type = 'int', value = '8', instLoc = loc(file, 4) },
    { kind = 'InstanceArray', instName = 'u', declName = 'leaf_mod', instLoc = loc(file, 5), declLoc = loc(file, 20) },
  }
  local array_kids = {
    { kind = 'Instance', instName = '[0]', declName = 'leaf_mod', instLoc = loc(file, 5), declLoc = loc(file, 20) },
    { kind = 'Instance', instName = '[1]', declName = 'leaf_mod', instLoc = loc(file, 5), declLoc = loc(file, 20) },
  }
  local leaf_kids = {
    { kind = 'Port', instName = 'd', type = 'input logic', instLoc = loc(file, 21) },
    { kind = 'Logic', instName = 'r', type = 'logic', instLoc = loc(file, 22) },
  }
  local roots = {
    { kind = 'Instance', instName = 'top', declName = 'top_mod', instLoc = loc(file, 0), declLoc = loc(file, 10) },
    { kind = 'Package', instName = 'pkg', instLoc = loc(file, 30) },
  }
  return {
    [''] = roots,
    ['top'] = top_kids,
    ['top.u'] = array_kids,
    ['top.u[0]'] = leaf_kids,
    ['top.u[1]'] = {},
    ['pkg'] = { { kind = 'Param', instName = 'x', type = 'int', value = '3', instLoc = loc(file, 31) } },
  }
end

-- The path as steps: "", then each prefix the plugin's rules make.
local function steps(scopes, path)
  local chain = { '' }
  local prefixes = {
    ['top'] = { 'top' },
    ['top.u'] = { 'top', 'top.u' },
    ['top.u[0]'] = { 'top', 'top.u', 'top.u[0]' },
    ['top.u[0].d'] = { 'top', 'top.u', 'top.u[0]', 'top.u[0].d' },
    ['top.u[1]'] = { 'top', 'top.u', 'top.u[1]' },
    ['pkg'] = { 'pkg' },
    ['pkg::x'] = { 'pkg', 'pkg::x' },
    ['top.nope.x'] = { 'top' },
  }
  -- (`SLANG.prefixes`: more paths for a test that adds nodes.)
  for key, value in pairs(SLANG.prefixes or {}) do
    prefixes[key] = value
  end
  for _, p in ipairs(prefixes[path] or {}) do
    table.insert(chain, p)
  end
  local out = {}
  for _, p in ipairs(chain) do
    table.insert(out, { path = p, children = scopes[p] or {} })
  end
  return out
end

-- The signal trace's three commands, from `SLANG` (shared by the fake slang-server and the fake svtrace).
local TRACE_COMMANDS = {
  ['slang.getDriversWithLocation'] = true,
  ['slang.getDriversWithConstants'] = true,
  ['slang.getLoadsWithLocation'] = true,
}

local function trace_answer(file, command, args)
  local graph = command == 'slang.getLoadsWithLocation' and SLANG.loads or SLANG.drivers
  -- (A plain string, as the real server takes it: anything else isn't found.)
  local path = args[1]
  local list = type(path) == 'string' and graph[path]
  if not list then
    return { error = 'Could not find path in compiled design: ' .. tostring(path) }
  end
  local out = {}
  for _, p in ipairs(list) do
    local entry = { path = p, location = loc(file, SLANG.lines[p] or 12) }
    -- (`SLANG.sites`: left out by default, as a server without the patch answers.)
    if SLANG.sites and SLANG.sites[p] then
      entry.sites = {}
      for i, site in ipairs(SLANG.sites[p]) do
        entry.sites[i] = { location = loc(file, site.line), kind = site.kind }
      end
    end
    table.insert(out, entry)
  end
  if command == 'slang.getDriversWithConstants' then
    -- (`SLANG.constants`: path -> lines.)
    local constants = {}
    for i, line in ipairs(SLANG.constants and SLANG.constants[path] or {}) do
      constants[i] = { location = loc(file, line), kind = 'constant' }
    end
    return { drivers = out, constants = constants }
  end
  return out
end

function _G.fake_slang(opts)
  opts = opts or {}
  local file = vim.api.nvim_buf_get_name(0)
  local scopes = design(file)
  SLANG.scopes = scopes
  local places = { ['top'] = 0, ['top.u'] = 5, ['top.u[0]'] = 5, ['top.u[1]'] = 5, ['pkg'] = 30 }
  FAKE.slang = FAKE.slang or {}
  local function answer(params)
    local command, args = params.command, params.arguments or {}
    if SLANG.fail then
      return { error = SLANG.fail }
    end
    if command == 'slang.getScopes' then
      return steps(scopes, args[1])
    elseif command == 'slang.getScope' then
      return vim.deepcopy(scopes[args[1]] or {})
    elseif TRACE_COMMANDS[command] then
      return trace_answer(file, command, args)
    elseif command == 'slang.getModulesInFile' then
      return vim.deepcopy(SLANG.modules or { 'leaf_mod' })
    elseif command == 'slang.getInstancesOfModule' then
      -- (The real server's { instPath, instLoc }; `SLANG.module_instances`: module -> paths.)
      local out = {}
      for _, path in ipairs((SLANG.module_instances or { leaf_mod = { 'top.u[0]', 'top.u[1]' } })[args[1]] or {}) do
        table.insert(out, { instPath = path, instLoc = loc(file, 5) })
      end
      return out
    elseif command == 'slang.getInstances' then
      return vim.deepcopy(SLANG.instances[args[1] and args[1].position and args[1].position.line] or {})
    elseif command == 'slang.searchHierarchy' then
      -- (Plain substring, as a test needs: the matches in the server's words.)
      local out = {}
      for _, path in ipairs({ 'top.clk', 'top.u[0]', 'top.u[1]', 'pkg::x' }) do
        if path:find(args[1] or '', 1, true) then
          table.insert(out, { path = path, description = 'a match' })
        end
      end
      return { matches = out }
    elseif command == 'slang.setBuildFile' or command == 'slang.setTopLevel' then
      -- (`SLANG.build_fail`: the server refuses.)
      return SLANG.build_fail and { error = SLANG.build_fail } or vim.NIL
    elseif command == 'slang.activateInstance' or command == 'slang.openWaveform' or command == 'slang.addToWaveform' then
      return vim.NIL
    elseif command == 'slang.setActiveInstance' then
      return vim.NIL
    elseif command == 'slang.showHierLocation' then
      local line = places[args[1] and args[1].hierPath]
      if line then
        vim.schedule(function()
          vim.lsp.util.show_document(loc(file, line), 'utf-16', { focus = true })
        end)
      end
      return vim.NIL
    end
    return { error = 'unknown command ' .. tostring(command) }
  end
  FAKE.slang['workspace/executeCommand'] = function(params)
    local value = answer(params)
    if SLANG.delay then
      return { delay = SLANG.delay, value = value }
    end
    return value
  end
  -- The code lens "select the active instance" on line 1 (0-based): `slang.quickPick` with two instances.
  FAKE.slang['textDocument/codeLens'] = {
    {
      range = { start = { line = 1, character = 0 }, ['end'] = { line = 1, character = 5 } },
      command = {
        title = 'top (2)',
        command = 'slang.quickPick',
        arguments = { {
          placeholder = 'Select active instance',
          interactionSource = 'codeLensSelect',
          onSelectCommand = 'slang.activateInstance',
          items = { { label = 'top.u[0]', description = 'leaf_mod', value = 'top.u[0]' }, { label = 'top.u[1]', value = 'top.u[1]' } },
        } },
      },
    },
  }
  local caps = {
    executeCommandProvider = {
      commands = {
        'slang.searchHierarchy', 'slang.setBuildFile', 'slang.setTopLevel', 'slang.activateInstance', 'slang.openWaveform',
        'slang.addToWaveform',
        'slang.getScope', 'slang.getScopes', 'slang.setActiveInstance', 'slang.showHierLocation',
        'slang.getDriversWithLocation', 'slang.getDriversWithConstants', 'slang.getLoadsWithLocation', 'slang.getInstances',
        'slang.getModulesInFile', 'slang.getInstancesOfModule',
      },
    },
  }
  return fake_lsp('slang', { caps = caps, server_info = { name = 'slang-server', version = '0.3.1' } })
end

-- ---- svtrace.step and svtrace.endpoints (SV_TRACE_PLAN.md, "Phase 2 spec"), from the same graph ----

local MANY, MANY_INSTANCES, MAX_DEPTH = 8, 2, 24

local function is_constant_path(path)
  return path:match('^[%w_$]+::[%w_$]+%.[%w_$]+$') ~= nil or (SLANG.constant_paths ~= nil and SLANG.constant_paths[path] == true)
end

local function instance_of(path)
  return path:match('^(.*)%.[^%.]*$') or path:match('^(.*)::[^:]*$') or ''
end

local function clock_only(path)
  local sites = SLANG.sites and SLANG.sites[path]
  if not sites or #sites == 0 then
    return false
  end
  for _, site in ipairs(sites) do
    if site.kind ~= 'clock' then
      return false
    end
  end
  return true
end

-- The paths one step from `path`, sorted and once each; nil when the graph doesn't know it.
local function step_paths(graph, path)
  local list = graph[path]
  if not list then
    return nil
  end
  local seen, out = {}, {}
  for _, p in ipairs(list) do
    if not seen[p] then
      seen[p] = true
      table.insert(out, p)
    end
  end
  table.sort(out)
  return out
end

-- How many of `paths` are signals: not a constant and, for drivers, not only a clock.
local function signal_paths(direction, paths)
  local out = {}
  for _, p in ipairs(paths) do
    if not is_constant_path(p) and not (direction == 'drivers' and clock_only(p)) then
      table.insert(out, p)
    end
  end
  return out
end

-- An entry's path without the instance its signal is in and the separator after it (`Row.local` in walk.rs).
local function local_of(path)
  local signal = SLANG.signals and SLANG.signals[path] or path
  local instance = instance_of(signal)
  if instance == '' or path:sub(1, #instance) ~= instance then
    return path
  end
  return (path:sub(#instance + 1):gsub('^%.', ''):gsub('^::', ''))
end

local function site_list(path)
  return SLANG.sites and SLANG.sites[path] or {}
end

local function preferred(prefer, path)
  return type(prefer) == 'string' and path:sub(1, #prefer + 1) == prefer .. '.'
end

-- The merged step of `starts` (`Walker::merged` in walk.rs): one row per entry of a single start, in its order;
-- of several, the union of their entries in rows by (local name, place, constant, the k-th of that in its start),
-- sorted by local name, then path. A row: { paths (the preferred first), from, sites = { { line, kind } }, constant }.
local function merged_rows(graph, starts, prefer)
  local rows = {}
  if #starts == 1 then
    for _, p in ipairs(step_paths(graph, starts[1]) or {}) do
      table.insert(rows, { paths = { p }, from = { starts[1] }, sites = vim.deepcopy(site_list(p)), constant = is_constant_path(p), name = local_of(p) })
    end
    return rows
  end
  local by, order = {}, {}
  for _, start in ipairs(starts) do
    local nth = {}
    for _, p in ipairs(step_paths(graph, start) or {}) do
      local base = local_of(p) .. '\0' .. (SLANG.lines[p] or 12) .. '\0' .. tostring(is_constant_path(p))
      nth[base] = (nth[base] or 0) + 1
      local key = base .. '\0' .. nth[base]
      local row = by[key]
      if not row then
        row = { paths = {}, from = {}, sites = {}, constant = is_constant_path(p), name = local_of(p) }
        by[key] = row
        table.insert(order, row)
      end
      if not vim.tbl_contains(row.paths, p) then
        table.insert(row.paths, p)
      end
      for _, site in ipairs(site_list(p)) do
        local have = false
        for _, other in ipairs(row.sites) do
          have = have or (other.line == site.line and other.kind == site.kind)
        end
        if not have then
          table.insert(row.sites, vim.deepcopy(site))
        end
      end
      if not vim.tbl_contains(row.from, start) then
        table.insert(row.from, start)
      end
    end
  end
  for _, row in ipairs(order) do
    table.sort(row.paths, function(a, b)
      local pa, pb = preferred(prefer, a), preferred(prefer, b)
      if pa ~= pb then
        return pa
      end
      return a < b
    end)
  end
  table.sort(order, function(a, b)
    if a.name ~= b.name then
      return a.name < b.name
    end
    return a.paths[1] < b.paths[1]
  end)
  return order
end

local function row_clock_only(row)
  if #row.sites == 0 then
    return false
  end
  for _, site in ipairs(row.sites) do
    if site.kind ~= 'clock' then
      return false
    end
  end
  return true
end

-- Rows that are signals: not a constant and, for drivers, not only a clock.
local function signal_rows(direction, rows)
  local out = {}
  for _, row in ipairs(rows) do
    if not row.constant and not (direction == 'drivers' and row_clock_only(row)) then
      table.insert(out, row)
    end
  end
  return out
end

-- Whether an answer is "many", as the real server has it (`Walker::many` in crates/svtrace/src/walk.rs): more
-- than MANY signals, in MANY_INSTANCES instances or more. It is a property of the answer an entry is in.
local function is_many_rows(direction, rows)
  local signals = signal_rows(direction, rows)
  local instances, count = {}, 0
  for _, row in ipairs(signals) do
    local instance = instance_of(SLANG.signals and SLANG.signals[row.paths[1]] or row.paths[1])
    if not instances[instance] then
      instances[instance] = true
      count = count + 1
    end
  end
  return #signals > MANY and count >= MANY_INSTANCES
end

local function step_entries(file, graph, direction, starts, prefer, chain, depth, follow)
  local out = {}
  local rows = merged_rows(graph, starts, prefer)
  -- (No entry of a many-answer is followed, whatever its own next step is.)
  local is_many = is_many_rows(direction, rows)
  for _, row in ipairs(rows) do
    local p = row.paths[1]
    local entry = { path = p, location = loc(file, SLANG.lines[p] or 12), paths = row.paths, from = row.from }
    -- (`SLANG.sites`: a path with none has none, as a server without them.)
    if SLANG.sites and SLANG.sites[p] then
      entry.sites = {}
      for i, site in ipairs(row.sites) do
        entry.sites[i] = { location = loc(file, site.line), kind = site.kind }
      end
    end
    -- (`SLANG.signals`: selected path -> its plain signal path, for the selected paths a test puts in the graph.)
    entry.signal = SLANG.signals and SLANG.signals[p] or p
    entry.constant = row.constant
    entry.loop = false
    for _, q in ipairs(row.paths) do
      entry.loop = entry.loop or chain[q] == true
    end
    local own = merged_rows(graph, row.paths, prefer)
    entry.total, entry.count = #own, #signal_rows(direction, own)
    if follow and not entry.loop and not entry.constant and not is_many and depth < MAX_DEPTH and entry.count == 1 then
      local inner = setmetatable({}, { __index = chain })
      for _, q in ipairs(row.paths) do
        inner[q] = true
      end
      entry.next = step_entries(file, graph, direction, row.paths, prefer, inner, depth + 1, follow)
    end
    table.insert(out, entry)
  end
  return out
end

-- The paths a step or an endpoints question starts from: `paths` (distinct) when the fake advertises them, else `path`.
local function start_paths(graph, arg, opts)
  local starts = {}
  if opts.paths ~= false and type(arg.paths) == 'table' then
    for _, p in ipairs(arg.paths) do
      if not vim.tbl_contains(starts, p) then
        table.insert(starts, p)
      end
    end
  elseif type(arg.path) == 'string' then
    starts = { arg.path }
  end
  if #starts == 0 then
    return nil, { error = 'svtrace.step takes { path, direction } as its argument' }
  end
  for _, p in ipairs(starts) do
    if type(p) ~= 'string' or not graph[p] then
      return nil, { error = 'Could not find path in compiled design: ' .. tostring(p) }
    end
  end
  return starts
end

local function svtrace_step(file, arg, opts)
  arg = type(arg) == 'table' and arg or {}
  local graph = arg.direction == 'loads' and SLANG.loads or SLANG.drivers
  local starts, err = start_paths(graph, arg, opts)
  if not starts then
    return err
  end
  local chain = {}
  for _, p in ipairs(starts) do
    chain[p] = true
  end
  for _, p in ipairs(arg.ancestors or {}) do
    chain[p] = true
  end
  -- (`follow` is on unless it is turned off, as in the real server.)
  return step_entries(file, graph, arg.direction, starts, arg.prefer, chain, 1, arg.follow ~= false)
end

-- The walk of `collect_endpoints` (slang_trace.lua), in one answer; of several starts, each one's walk in turn.
local function svtrace_endpoints(file, arg, opts)
  arg = type(arg) == 'table' and arg or {}
  local loads = arg.direction == 'loads'
  local graph = loads and SLANG.loads or SLANG.drivers
  local starts, err = start_paths(graph, arg, opts)
  if not starts then
    return err
  end
  local lines, found = {}, {}
  local function add(location)
    local key = location.uri .. ':' .. location.range.start.line
    if not lines[key] then
      lines[key] = true
      table.insert(found, location)
    end
  end
  for _, start in ipairs(starts) do
    local seen = { [start] = true }
    local frontier = { { path = start } }
    while #frontier > 0 do
      local following = {}
      for _, item in ipairs(frontier) do
        local paths = step_paths(graph, item.path) or {}
        for _, p in ipairs(paths) do
          local sites = (not is_constant_path(p) and SLANG.sites and SLANG.sites[p]) or {}
          for _, site in ipairs(sites) do
            if site.kind == 'port' or site.kind == 'wire' then
              if not seen[p] then
                seen[p] = true
                table.insert(following, { path = p, location = loc(file, SLANG.lines[p] or 12) })
              end
            elseif site.kind == 'data' or (site.kind == 'control' or site.kind == 'clock') and loads then
              add(loc(file, site.line))
            end
          end
        end
        local constants = (not loads and SLANG.constants and SLANG.constants[item.path]) or {}
        for _, line in ipairs(constants) do
          add(loc(file, line))
        end
        if #paths == 0 and #constants == 0 and item.location and item.path:match('^[^%.:]+%.[^%.:]+$') then
          add(item.location)
        end
      end
      frontier = following
    end
  end
  return found
end

-- `svtrace.atCursor` (SV_TRACE_PLAN.md, "Phase 3 spec"): the selected paths of the expression at the cursor,
-- from `SLANG.at_cursor` (cursor line, 0-based -> paths, answered as `{ path, signal }`; `SLANG.at_cursor_calls` keeps the arguments it got), else
-- from `SLANG.instances` (so the cursor tests of the other fakes answer the same).
local function svtrace_at_cursor(arg)
  SLANG.at_cursor_calls = SLANG.at_cursor_calls or {}
  table.insert(SLANG.at_cursor_calls, vim.deepcopy(arg))
  local line = type(arg) == 'table' and type(arg.position) == 'table' and arg.position.line
  if type(arg) ~= 'table' or type(arg.textDocument) ~= 'table' or type(arg.textDocument.uri) ~= 'string' or not line then
    return { error = 'svtrace.atCursor takes { textDocument = { uri }, position }' }
  end
  -- ({ path, signal }: the plain signal is `SLANG.signals[path]`, else the path.)
  local out = {}
  for i, path in ipairs((SLANG.at_cursor and SLANG.at_cursor[line]) or SLANG.instances[line] or {}) do
    out[i] = { path = path, signal = SLANG.signals and SLANG.signals[path] or path }
  end
  return out
end

-- A fake svtrace: the same three commands from the same graph, and, unless `opts.phase1`, `svtrace.step`
-- and `svtrace.endpoints` (taking `paths`, and saying so, unless `opts.paths == false`), and `svtrace.atCursor` unless `opts.no_cursor`, and with `opts.scopes` the hierarchy's `slang.getScope` and `slang.getScopes`. (Everything else is the real slang-server's, so it answers nothing else.)
function _G.fake_svtrace(opts)
  opts = opts or {}
  local file = vim.api.nvim_buf_get_name(0)
  FAKE.svtrace = FAKE.svtrace or {}
  FAKE.svtrace['workspace/executeCommand'] = function(params)
    -- (`SLANG.svtrace_fail`: only svtrace fails, as with no design it can build.)
    if SLANG.fail or SLANG.svtrace_fail then
      return { error = SLANG.fail or SLANG.svtrace_fail }
    end
    if SLANG.svtrace_no_design and params.command ~= 'svtrace.status' and params.command ~= 'svtrace.useBuild' then
      return { error = 'svtrace: no design: no build chosen' }
    end
    if opts.scopes and params.command == 'slang.getScopes' then
      return steps(SLANG.scopes or design(file), (params.arguments or {})[1])
    elseif opts.scopes and params.command == 'slang.getScope' then
      return vim.deepcopy((SLANG.scopes or design(file))[(params.arguments or {})[1]] or {})
    end
    if not opts.phase1 and params.command == 'svtrace.step' then
      return svtrace_step(file, (params.arguments or {})[1], opts)
    elseif not opts.phase1 and params.command == 'svtrace.endpoints' then
      return svtrace_endpoints(file, (params.arguments or {})[1], opts)
    elseif not opts.phase1 and not opts.no_cursor and params.command == 'svtrace.atCursor' then
      return svtrace_at_cursor((params.arguments or {})[1])
    end
    if params.command == 'svtrace.status' then
      -- (`SLANG.svtrace_no_design`: svtrace has no design, as after `useBuild { file = null }`.)
      return { state = SLANG.svtrace_no_design and 'failed' or 'ready', design = not SLANG.svtrace_no_design }
    elseif params.command == 'svtrace.useBuild' then
      local file = ((params.arguments or {})[1] or {}).file
      SLANG.svtrace_no_design = file == nil or file == vim.NIL
      return { state = SLANG.svtrace_no_design and 'failed' or 'ready', design = not SLANG.svtrace_no_design }
    elseif params.command == 'svtrace.reload' then
      -- (The real one elaborates again and answers with the build's status.)
      return { state = 'ready' }
    end
    if not TRACE_COMMANDS[params.command] then
      return { error = 'unknown command ' .. tostring(params.command) }
    end
    return trace_answer(file, params.command, params.arguments or {})
  end
  local commands = {
    'slang.getDriversWithLocation', 'slang.getDriversWithConstants', 'slang.getLoadsWithLocation',
    'svtrace.reload', 'svtrace.status', 'svtrace.listSignals', 'svtrace.useBuild',
  }
  if not opts.phase1 then
    table.insert(commands, 'svtrace.step')
    table.insert(commands, 'svtrace.endpoints')
    if not opts.no_cursor then
      table.insert(commands, 'svtrace.atCursor')
    end
  end
  if opts.scopes then
    -- (`opts.scopes`: the hierarchy's two commands too, from the same design as the fake slang-server's.)
    table.insert(commands, 'slang.getScope')
    table.insert(commands, 'slang.getScopes')
  end
  local caps = { executeCommandProvider = { commands = commands } }
  if not opts.phase1 and opts.paths ~= false then
    -- (Merged instances: `paths` and `prefer` in step and endpoints, as the real one says.)
    caps.experimental = { svtrace = { paths = true } }
  end
  return fake_lsp('svtrace', { caps = caps, server_info = { name = 'svtrace', version = '0.1.0' } })
end
