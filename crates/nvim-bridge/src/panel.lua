-- The bottom panel's Neovim side. The panel itself (Problems, the Git output and terminals) is the GUI's own,
-- drawn in the window beside Neovim's grid; Neovim has no panel windows, and :copen is its own
-- quickfix window. The commands keep their arguments but only tell the GUI, as :EshviExplorer
-- does, and the diagnostics are reported for the Problems list (`gui_problems`).
-- Run by eshvi after setup.lua, with the same arguments: the GUI's channel id and its options.
local chan, opts = ...
opts = opts or {}
if not opts.terminal_enabled then
  return
end

_G.Eshvi = _G.Eshvi or {}

local function tell(kind, args)
  vim.rpcnotify(chan, 'gui_panel', { kind = kind, args = args })
end
vim.api.nvim_create_user_command('EshviPanel', function(o)
  tell('panel', o.fargs)
end, {
  nargs = '?',
  complete = function()
    return { 'toggle', 'show', 'hide' }
  end,
  desc = 'eshvi: show or hide the bottom panel',
})
vim.api.nvim_create_user_command('EshviTerminal', function(o)
  tell('terminal', o.fargs)
end, {
  nargs = '*',
  complete = function()
    return { 'toggle', 'show', 'hide', 'new', 'next', 'prev', 'close', 'select', 'split', 'height' }
  end,
  desc = 'eshvi: the bottom panel\'s terminals',
})
-- :EshviProblems [toggle|show|hide] [file|workspace] | severity E|W|I | filter {text}
vim.api.nvim_create_user_command('EshviProblems', function(o)
  local args = o.fargs
  if args[1] == 'filter' then
    -- (The text is what follows, spaces and all.)
    args = { 'filter', vim.trim(o.args:sub(#'filter' + 1)) }
  end
  tell('problems', args)
end, {
  nargs = '*',
  complete = function()
    return { 'toggle', 'show', 'hide', 'file', 'workspace', 'severity', 'filter' }
  end,
  desc = 'eshvi: the bottom panel\'s Problems list',
})
-- :EshviGitOutput [toggle|show|hide]: what the git commands of Source Control said.
vim.api.nvim_create_user_command('EshviGitOutput', function(o)
  tell('git_output', o.fargs)
end, {
  nargs = '?',
  complete = function()
    return { 'toggle', 'show', 'hide' }
  end,
  desc = 'eshvi: the bottom panel\'s Git output',
})

-- Every diagnostic (hints left out, as in VS Code's Problems) for the GUI's list, after
-- they have stopped changing for 200 ms. Lines and columns count from 1.
local timer = vim.uv.new_timer()
local function report_problems()
  local items = {}
  local found = vim.diagnostic.get(nil, { severity = { min = vim.diagnostic.severity.INFO } })
  for _, d in ipairs(found) do
    local name = vim.api.nvim_buf_is_valid(d.bufnr) and vim.api.nvim_buf_get_name(d.bufnr) or ''
    if name ~= '' then
      table.insert(items, {
        file = vim.fn.fnamemodify(name, ':p'),
        lnum = d.lnum + 1,
        col = d.col + 1,
        end_lnum = (d.end_lnum or d.lnum) + 1,
        end_col = (d.end_col or d.col) + 1,
        severity = d.severity,
        message = d.message or '',
        source = d.source or '',
        code = d.code ~= nil and tostring(d.code) or '',
      })
    end
  end
  vim.rpcnotify(chan, 'gui_problems', { items = items })
end
vim.api.nvim_create_autocmd('DiagnosticChanged', {
  group = vim.api.nvim_create_augroup('eshvi_problems', { clear = true }),
  callback = function()
    timer:stop()
    timer:start(200, 0, vim.schedule_wrap(report_problems))
  end,
})
