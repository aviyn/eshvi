-- `:checkhealth eshvi`: which plugins open their own popups. The app writes this
-- file to `<stdpath('cache')>/eshvi/runtime/lua/eshvi/health.lua` at each start
-- and adds that folder to the runtimepath, because `:checkhealth` only looks for
-- `lua/**/health.lua` there. It reads what `float_log.lua` recorded
-- (`_G.EshviFloatLog`), with a code search as the smaller second part. Every look
-- at another plugin is in pcall: the report never errors.
local M = {}

local MENUS = {
  'which-key', 'telescope', 'fzf-lua', 'snacks', 'nui', 'cmp', 'blink.cmp', 'noice', 'notify', 'dressing', 'wilder',
}
local OWN_WINDOWS = {
  'lazy', 'oil', 'gitsigns', 'flash', 'render-markdown', 'nvim-tree.lua', 'plenary', 'slang-server',
}

-- "which-key.nvim", "nvim-cmp" and "which-key" are the same plugin to this check.
local function norm(name)
  name = tostring(name):lower():gsub('%.nvim$', ''):gsub('%-nvim$', ''):gsub('^nvim%-', '')
  return name
end

local function set(list)
  local out = {}
  for _, name in ipairs(list) do
    out[norm(name)] = true
  end
  return out
end

local menus, own_windows = set(MENUS), set(OWN_WINDOWS)

local function is_menu_plugin(name)
  local n = norm(name)
  return menus[n] or n == 'mini' or n:find('^mini%.') ~= nil
end

local function plural(n, one, many)
  return ('%d %s'):format(n, n == 1 and one or many)
end

local function guard(fn)
  local ok, err = pcall(fn)
  if not ok then
    vim.health.warn('This part of the check failed: ' .. tostring(err))
  end
end

local function short(file, name)
  if type(file) ~= 'string' then
    return '?'
  end
  local _, j = file:find('/' .. name .. '/', 1, true)
  if j then
    return file:sub(j + 1)
  end
  return vim.fn.fnamemodify(file, ':~')
end

local function top_site(o, name)
  local best, best_n
  for site, n in pairs(o.sites or {}) do
    if not best_n or n > best_n or (n == best_n and site < best) then
      best, best_n = site, n
    end
  end
  if not best then
    return nil
  end
  local file, line = best:match('^(.*):(%d+)$')
  return ('%s:%s'):format(short(file or best, name), line or '?')
end

local function allow_set()
  local list = vim.g.eshvi_health_allow
  return type(list) == 'table' and set(vim.tbl_filter(function(x)
    return type(x) == 'string'
  end, list)) or {}
end

local ADVICE = { 'If this is fine, add it to vim.g.eshvi_health_allow' }

local function ago(t)
  local s = math.max(0, os.time() - t)
  if s < 90 then
    return s .. ' s'
  elseif s < 5400 then
    return math.floor(s / 60 + 0.5) .. ' min'
  end
  return ('%.1f h'):format(s / 3600)
end

local function popups(log)
  vim.health.start('Popups seen in this session')
  if vim.g.eshvi_health == false then
    vim.health.info('Not recording: turned off by vim.g.eshvi_health = false')
    return
  end
  local allowed = allow_set()
  local names = {}
  for name in pairs(log.owners) do
    if name ~= 'Eshvi' then
      table.insert(names, name)
    end
  end
  table.sort(names, function(a, b)
    local x, y = log.owners[a].count, log.owners[b].count
    if x ~= y then
      return x > y
    end
    return a < b
  end)
  for _, name in ipairs(names) do
    local o = log.owners[name]
    local n = norm(name)
    local count = plural(o.count, 'popup', 'popups')
    local site = top_site(o, name)
    local where = site and (', most from ' .. site) or ''
    if allowed[n] then
      vim.health.ok(('%s: %s (allowed by vim.g.eshvi_health_allow)'):format(name, count))
    elseif is_menu_plugin(name) then
      vim.health.warn(('%s: %s%s. It is a menu plugin'):format(name, count, where), ADVICE)
    elseif own_windows[n] or name == 'Neovim' or name == 'your config' then
      local what = name == 'Neovim' and 'Neovim itself, for instance a diagnostic float'
        or name == 'your config' and 'code in your config or typed with :lua'
        or 'its own window, not a menu'
      vim.health.info(('%s: %s (%s)'):format(name, count, what))
    elseif o.menus > 0 then
      vim.health.warn(('%s: %s%s'):format(name, count, where), ADVICE)
    else
      vim.health.info(('%s: %s, all non-focusable overlays%s'):format(name, count, where))
    end
  end
  if log.unknown > 0 then
    vim.health.info(
      ('%s opened from Vimscript or before the check started'):format(plural(log.unknown, 'float was', 'floats were'))
    )
  end
  if #names == 0 and log.unknown == 0 then
    vim.health.info(
      ('No plugin has opened a float since Neovim started (%s ago). Use the editor, then run this again.'):format(
        ago(log.started)
      )
    )
  end
end

local function owner_name(log, fn)
  local info = debug.getinfo(fn, 'S')
  local owner, kind = log.owner_of(info.source)
  return owner or 'Neovim', info.source
end

local function replaced(log)
  vim.health.start('Menus Eshvi replaces')
  local picker = rawget(_G, 'Eshvi') and Eshvi.picker

  if picker and vim.ui.select == picker.select then
    vim.health.ok("vim.ui.select: Eshvi's native picker")
  else
    local owner = owner_name(log, vim.ui.select)
    if owner == 'Neovim' and vim.g.eshvi_native_picker == false then
      vim.health.info("vim.ui.select: Neovim's own, the native picker is turned off by vim.g.eshvi_native_picker = false")
    else
      vim.health.warn(
        ('vim.ui.select: replaced by %s, so Eshvi\'s picker is not used for it'):format(owner),
        { "Turn off that plugin's select, or remove it" }
      )
    end
  end

  local owner = owner_name(log, vim.ui.input)
  if owner == 'Neovim' then
    vim.health.ok("vim.ui.input: Neovim's own (the command line)")
  else
    vim.health.warn(
      ('vim.ui.input: replaced by %s, which draws its own popup'):format(owner),
      { "Turn off that plugin's input, or remove it" }
    )
  end

  if vim.notify == log.notify then
    local _, source = owner_name(log, vim.notify)
    if source:sub(1, 4) == 'vim/' then
      vim.health.ok("vim.notify: Neovim's own (messages)")
    else
      vim.health.ok("vim.notify: Eshvi's (messages, shown as toasts)")
    end
  else
    local o = owner_name(log, vim.notify)
    if o == 'Neovim' then
      vim.health.ok("vim.notify: Neovim's own (messages)")
    else
      vim.health.warn(
        ('vim.notify: replaced by %s, which draws its own popup'):format(o),
        { "Turn off that plugin's notify, or remove it; [messages] restore_notify = true keeps Eshvi's" }
      )
    end
  end
end

local function lazy_plugins()
  if not package.loaded['lazy'] then
    return nil
  end
  local ok, list = pcall(function()
    return require('lazy').plugins()
  end)
  return ok and type(list) == 'table' and list or nil
end

-- What of `names` (plugin names) and `modules` is there: { 'name (loaded)' ... }.
local function find_plugin(plugins, names, modules)
  local wanted = set(names)
  for _, p in ipairs(plugins or {}) do
    if type(p) == 'table' and p.name and wanted[norm(p.name)] then
      return p.name, (p._ and p._.loaded) and 'loaded' or 'installed, not loaded yet'
    end
  end
  for _, m in ipairs(modules) do
    if package.loaded[m] then
      return names[1], 'loaded'
    end
  end
end

local function native_features()
  vim.health.start('Native features')
  local plugins = lazy_plugins()

  if vim.g.eshvi_native_completion == false then
    vim.health.info('Completion: turned off by vim.g.eshvi_native_completion = false')
  else
    vim.health.ok("Completion: Neovim's native menu, drawn by Eshvi")
  end
  for _, c in ipairs({
    { 'blink.cmp', { 'blink.cmp' }, { 'blink.cmp' } },
    { 'nvim-cmp', { 'nvim-cmp' }, { 'cmp' } },
    { 'coq_nvim', { 'coq_nvim' }, { 'coq' } },
  }) do
    local name, how = find_plugin(plugins, c[2], c[3])
    if name then
      vim.health.warn(
        ('Completion: %s is %s. Eshvi draws completion itself and ignores its menu'):format(c[1], how),
        { 'Remove it from the config' }
      )
    end
  end

  if vim.g.eshvi_lsp_popups == false then
    vim.health.info('LSP popups: turned off by vim.g.eshvi_lsp_popups = false')
  else
    vim.health.ok('LSP popups: hover (K), signature help and diagnostics drawn by Eshvi')
  end

  if vim.g.eshvi_slang == false then
    vim.health.info('Slang: turned off by vim.g.eshvi_slang = false')
  else
    vim.health.ok('Slang: Hierarchy and Trace views, :EshviSlang (the views come with a slang-server or svtrace client)')
  end

  vim.health.ok('Status bar: drawn by Eshvi')
  for _, s in ipairs({
    { 'lualine.nvim', { 'lualine.nvim' }, { 'lualine' } },
    { 'heirline.nvim', { 'heirline.nvim' }, { 'heirline' } },
    { 'feline.nvim', { 'feline.nvim' }, { 'feline' } },
    { 'galaxyline.nvim', { 'galaxyline.nvim' }, { 'galaxyline' } },
    { 'windline.nvim', { 'windline.nvim' }, { 'windline' } },
    { 'staline.nvim', { 'staline.nvim' }, { 'staline' } },
  }) do
    local name, how = find_plugin(plugins, s[2], s[3])
    if name then
      vim.health.warn(
        ('Status bar: %s is %s. Eshvi ignores what it draws'):format(s[1], how),
        { 'Remove it from the config' }
      )
    end
  end
  if vim.g.loaded_lightline then
    vim.health.warn('Status bar: lightline is loaded. Eshvi ignores what it draws', { 'Remove it from the config' })
  end

  if vim.g.eshvi_native_picker == false then
    vim.health.info('Picker: turned off by vim.g.eshvi_native_picker = false')
  else
    vim.health.ok("Picker: Eshvi's native picker")
  end
  for _, c in ipairs({
    { 'telescope.nvim', { 'telescope.nvim' }, { 'telescope' } },
    { 'fzf-lua', { 'fzf-lua' }, { 'fzf-lua' } },
    { 'mini.pick', { 'mini.pick' }, { 'mini.pick' } },
  }) do
    local name, how = find_plugin(plugins, c[2], c[3])
    if name then
      vim.health.warn(
        ('Picker: %s is %s. It opens its own popups'):format(c[1], how),
        { 'Use Eshvi.picker.* instead, and remove it' }
      )
    end
  end
  local snacks = package.loaded['snacks']
  local ok, enabled = pcall(function()
    return snacks and snacks.config and snacks.config.picker and snacks.config.picker.enabled == true
  end)
  if ok and enabled then
    vim.health.warn('Picker: Snacks.picker is enabled. It opens its own popups', {
      'Set picker = { enabled = false } in snacks.nvim\'s opts, and use Eshvi.picker.*',
    })
  end
end

-- Plugin folders to read: lazy's loaded ones, else the runtimepath.
local function active_dirs(log)
  local out = {}
  local plugins = lazy_plugins()
  local skip = { 'eshvi', 'gui_bridge' }
  if plugins then
    for _, p in ipairs(plugins) do
      if type(p) == 'table' and p.dir and p.name and p._ and p._.loaded then
        table.insert(out, { name = p.name, dir = p.dir })
      end
    end
  else
    local runtime = vim.env.VIMRUNTIME and (vim.env.VIMRUNTIME:gsub('/+$', '')) or ''
    local config = vim.fn.stdpath('config')
    for _, dir in ipairs(vim.api.nvim_list_runtime_paths()) do
      local name = vim.fs.basename(dir)
      if name == 'after' then
        name = vim.fs.basename(vim.fs.dirname(dir))
      end
      if dir ~= config and dir ~= runtime and dir:sub(1, #runtime + 1) ~= runtime .. '/' and dir ~= vim.fn.stdpath('config') .. '/after' then
        table.insert(out, { name = name, dir = dir })
      end
    end
  end
  return vim.tbl_filter(function(e)
    return not vim.tbl_contains(skip, e.name) and not e.dir:find('/eshvi/runtime', 1, true)
  end, out)
end

local SKIP_DIRS = { tests = true, test = true, spec = true, doc = true, docs = true, ['.git'] = true, node_modules = true }

local function count_in(text)
  local n = 0
  for _, word in ipairs({ 'nvim_open_win', 'open_floating_preview' }) do
    local from = 1
    while true do
      local i, j = text:find(word, from, true)
      if not i then
        break
      end
      n, from = n + 1, j + 1
    end
  end
  return n
end

local function count_dir(dir, depth)
  local total = 0
  local handle = vim.uv.fs_scandir(dir)
  if not handle then
    return 0
  end
  while true do
    local name, kind = vim.uv.fs_scandir_next(handle)
    if not name then
      break
    end
    local path = dir .. '/' .. name
    if kind == 'directory' then
      if not SKIP_DIRS[name] and depth < 12 then
        total = total + count_dir(path, depth + 1)
      end
    elseif kind == 'file' and name:sub(-4) == '.lua' then
      local fd = vim.uv.fs_open(path, 'r', 438)
      if fd then
        local stat = vim.uv.fs_fstat(fd)
        if stat and stat.size < 2 * 1024 * 1024 then
          local text = vim.uv.fs_read(fd, stat.size, 0)
          if text then
            total = total + count_in(text)
          end
        end
        vim.uv.fs_close(fd)
      end
    end
  end
  return total
end

local function could_open(log)
  vim.health.start('Could open floats, not seen yet')
  vim.health.info(
    'This is only what the code could do: plugins whose Lua calls nvim_open_win or open_floating_preview and that have not opened a popup in this session.'
  )
  local seen = {}
  for name in pairs(log and log.owners or {}) do
    seen[norm(name)] = true
  end
  local found = {}
  for _, e in ipairs(active_dirs(log)) do
    if not seen[norm(e.name)] then
      local ok, n = pcall(count_dir, e.dir, 0)
      if ok and n > 0 then
        table.insert(found, { name = e.name, n = n })
      end
    end
  end
  table.sort(found, function(a, b)
    if a.n ~= b.n then
      return a.n > b.n
    end
    return a.name < b.name
  end)
  for _, f in ipairs(found) do
    vim.health.info(('%s: %s in its code'):format(f.name, plural(f.n, 'place', 'places')))
  end
  if #found == 0 then
    vim.health.info('No other active plugin has such code.')
  end
end

function M.check()
  local log = rawget(_G, 'EshviFloatLog')
  if type(log) ~= 'table' or type(log.owners) ~= 'table' then
    vim.health.start('Popups seen in this session')
    vim.health.info('Nothing was recorded: Eshvi started Neovim without the float log (a shared config does not have it)')
    guard(function()
      could_open(nil)
    end)
    return
  end
  guard(function()
    popups(log)
  end)
  guard(function()
    replaced(log)
  end)
  guard(native_features)
  guard(function()
    could_open(log)
  end)
end

return M
