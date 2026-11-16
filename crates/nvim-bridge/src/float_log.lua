-- Passed to nvim with --cmd (after early.lua), so it runs before the user's
-- config: plugins that keep `vim.api.nvim_open_win` in a local at load get the
-- wrapper too. Remembers which plugin opened which floating window, for
-- `:checkhealth eshvi` (`health.lua`). Only used inside eshvi, with the own
-- config. `vim.g.eshvi_health = false` stops the recording (read at the first
-- float). Full-line comments are stripped and lines joined, so no inline
-- comments and no double dashes anywhere else.
--
-- `_G.EshviFloatLog` = { owners = { [name] = { count, menus, overlays, file,
-- line, last, sites = { ['file:line'] = n } } }, unknown = n, nsites = n,
-- started = os.time(), notify = the vim.notify that was there at the start,
-- owner_of = function(source) }.
local api = vim.api
local orig = api.nvim_open_win
local MAX_SITES = 200
local log = { owners = {}, unknown = 0, nsites = 0, started = os.time(), notify = vim.notify }
_G.EshviFloatLog = log
local seen, counted = {}, {}
local state = {}
local cache = {}

local function off()
  if state.off == nil then
    state.off = vim.g.eshvi_health == false
  end
  return state.off
end

local function prefixes(paths)
  local out = {}
  for _, p in ipairs(paths) do
    if type(p) == 'string' and p ~= '' then
      p = (p:gsub('\\', '/'):gsub('/+$', '')) .. '/'
      table.insert(out, p)
    end
  end
  return out
end

local function dirs()
  if not state.dirs then
    local rt, cfg = vim.env.VIMRUNTIME, vim.fn.stdpath('config')
    state.dirs = {
      runtime = prefixes({ rt, rt and vim.uv.fs_realpath(rt) }),
      config = prefixes({ cfg, vim.uv.fs_realpath(cfg) }),
    }
  end
  return state.dirs
end

local function under(path, list)
  for _, p in ipairs(list) do
    if path:sub(1, #p) == p then
      return true
    end
  end
  return false
end

local function owner_of(source)
  local c = cache[source]
  if c then
    return c[1], c[2]
  end
  local owner, kind
  if source:sub(1, 1) ~= '@' then
    if source == '<nvim>' then
      owner = 'Eshvi'
    elseif source:sub(1, 4) == 'vim/' then
      kind = 'runtime'
    elseif source:sub(1, 1) == '=' then
      kind = 'skip'
    else
      owner = 'your config'
    end
  else
    local p = (source:sub(2):gsub('\\', '/'))
    local d = dirs()
    if p:find('/lua/gui_bridge[/.]') or p:find('/eshvi/runtime/lua/eshvi/', 1, true) then
      owner = 'Eshvi'
    elseif p:sub(1, 4) == 'vim/' or under(p, d.runtime) then
      kind = 'runtime'
    elseif under(p, d.config) then
      owner = 'your config'
    else
      owner = p:match('/lazy/([^/]+)/') or p:match('/pack/[^/]+/start/([^/]+)/') or p:match('/pack/[^/]+/opt/([^/]+)/') or vim.fs.basename(p)
    end
  end
  if #source < 400 then
    cache[source] = { owner, kind }
  end
  return owner, kind
end
log.owner_of = owner_of

local wrapper
local function walk()
  for level = 2, 60 do
    local info = debug.getinfo(level, 'Slf')
    if not info then
      break
    end
    if info.func ~= wrapper and info.func ~= walk and info.what ~= 'C' then
      local owner, kind = owner_of(info.source)
      if owner then
        return owner, info.source:sub(1, 1) == '@' and info.source:sub(2) or info.source, info.currentline
      end
    end
  end
  return 'Neovim'
end

local function is_float(cfg)
  return type(cfg) == 'table' and ((cfg.relative ~= nil and cfg.relative ~= '') or cfg.external == true)
end

local function store(owner, file, line, enter, cfg, win)
  if not owner or not is_float(cfg) then
    return
  end
  if type(win) == 'number' then
    seen[win] = true
  end
  local o = log.owners[owner]
  if not o then
    o = { count = 0, menus = 0, overlays = 0, file = file, line = line, last = 0, sites = {} }
    log.owners[owner] = o
  end
  o.count = o.count + 1
  o.last = os.time()
  if enter == true or cfg.focusable ~= false then
    o.menus = o.menus + 1
  else
    o.overlays = o.overlays + 1
  end
  if file then
    local key = file .. ':' .. tostring(line)
    if o.sites[key] or log.nsites < MAX_SITES then
      if not o.sites[key] then
        log.nsites = log.nsites + 1
      end
      o.sites[key] = (o.sites[key] or 0) + 1
    end
  end
end

local function finish(owner, file, line, enter, cfg, ok, ...)
  if not ok then
    error((...), 2)
  end
  if select('#', ...) > 0 then
    pcall(store, owner, file, line, enter, cfg, (...))
  end
  return ...
end

wrapper = function(...)
  if off() then
    return orig(...)
  end
  local ok, owner, file, line = pcall(walk)
  local _, enter, cfg = ...
  return finish(ok and owner or nil, file, line, enter, cfg, pcall(orig, ...))
end
rawset(api, 'nvim_open_win', wrapper)

local function scan()
  if off() then
    return
  end
  for _, win in ipairs(api.nvim_list_wins()) do
    if not seen[win] and not counted[win] then
      local ok, cfg = pcall(api.nvim_win_get_config, win)
      if ok and cfg.relative ~= '' then
        counted[win] = true
        log.unknown = log.unknown + 1
      end
    end
  end
end

local group = api.nvim_create_augroup('EshviFloatLog', { clear = true })
api.nvim_create_autocmd('WinNew', {
  group = group,
  callback = function()
    vim.schedule(function()
      pcall(scan)
    end)
  end,
})
api.nvim_create_autocmd('WinClosed', {
  group = group,
  callback = function(ev)
    local win = tonumber(ev.match)
    if win then
      seen[win], counted[win] = nil, nil
    end
  end,
})
