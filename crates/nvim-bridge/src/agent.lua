-- The agent panel's Neovim side. The panel is the GUI's own: the agents are processes on a PTY, a tab
-- each, in a column on the window's right (`app/agent_panel.rs`). :EshviAgent [toggle|show|hide|focus|
-- new [name]|resume|next|prev|close [id]|select id|restart] keeps its arguments but only tells the GUI,
-- as :EshviPanel does. What stays here is what the IDE connection (`ide.lua`) needs: the agents'
-- environment, the shown agent's process for "send to the agent", and opening files the agent mentions.
-- Arguments: the GUI's channel and the options table (agent_command, agent_commands, agent_env).
local chan, opts = ...

_G.Eshvi = _G.Eshvi or {}
local actions = { 'toggle', 'show', 'hide', 'focus', 'new', 'resume', 'next', 'prev', 'close', 'select', 'restart' }
vim.api.nvim_create_user_command('EshviAgent', function(o)
  local action = o.fargs[1] or 'toggle'
  if not vim.tbl_contains(actions, action) then
    return vim.notify('EshviAgent: unknown action ' .. action, vim.log.levels.ERROR)
  end
  vim.rpcnotify(chan, 'gui_panel', { kind = 'agent', args = o.fargs })
end, {
  nargs = '*',
  complete = function(_, line)
    if line:match('^%S+%s+new%s+') then
      local names = vim.tbl_keys(type(opts.agent_commands) == 'table' and opts.agent_commands or {})
      table.insert(names, vim.fs.basename((opts.agent_command or { 'claude' })[1]))
      table.sort(names)
      return names
    end
    return actions
  end,
  desc = 'eshvi: the agent panel',
})
-- The environment agents are started with (Claude Code's IDE connection).
vim.rpcnotify(chan, 'gui_agent_env', opts.agent_env or {})

-- The shown agent's process and folder, pushed by the GUI whenever they change.
local shown = {}
function Eshvi.agent_set_shown(pid, cwd)
  pid = tonumber(pid)
  shown = { pid = (pid and pid > 0) and pid or nil, cwd = (cwd and cwd ~= '') and cwd or nil }
end
function Eshvi.agent_pid()
  return shown.pid
end

-- Open a file the agent mentions (IDE `openFile`, `openDiff` of an existing file) in the
-- editor window; a relative path is from where the shown agent runs.
function Eshvi.agent_open(path, line, col)
  local full = vim.fs.normalize(path)
  if not full:match('^/') and not full:match('^%a:/') then
    local root = vim.g.eshvi_root
    if type(root) ~= 'string' or vim.fn.isdirectory(root) ~= 1 then
      root = vim.fn.getcwd()
    end
    full = vim.fs.joinpath(shown.cwd or root, full)
  end
  if vim.fn.filereadable(full) ~= 1 then
    return false
  end
  local win = Eshvi.editor_window and Eshvi.editor_window()
  if win then
    vim.api.nvim_set_current_win(win)
  end
  vim.cmd('drop ' .. vim.fn.fnameescape(full))
  if line and line > 0 then
    pcall(vim.api.nvim_win_set_cursor, 0, { math.min(line, vim.api.nvim_buf_line_count(0)), math.max((col or 1) - 1, 0) })
    vim.cmd('normal! zz')
  end
  return true
end
