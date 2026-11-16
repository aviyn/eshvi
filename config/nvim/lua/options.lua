-- vim.o.guifont = 'Hack Nerd Font:h15'
vim.o.guifont = 'JetBrainsMono Nerd Font:h16'

vim.o.termguicolors = true
vim.o.encoding = 'utf8'
vim.o.mouse = 'a'
-- vim.o.clipboard = 'unnamedplus'
vim.o.clipboard = 'unnamed'
vim.o.signcolumn ='yes:1'
vim.o.hidden = true
vim.o.scrolloff = 12     -- Keep 12 lines above or below the cursor
vim.o.wmh = 0            -- Minimal buffer window hight is 0
vim.o.laststatus = 2     -- Always show status bar
-- vim.o.laststatus = 3     -- Always show status bar
-- vim.o.cmdheight = 0      -- Don't show command line
vim.o.showmode = false   -- Don't show mode in the command line
-- vim.o.cursorline = true  -- Highlight current line
vim.o.number = true
vim.o.relativenumber = true
-- vim.o.wildmenu
vim.o.showmatch = true   -- Show the matching bracket
-- Indentation
vim.o.autoindent = true  -- Enable autoindent
-- Defaults
vim.o.expandtab = true
vim.o.shiftwidth = 2
vim.o.tabstop = 2
vim.o.softtabstop = 2

vim.o.splitbelow = true               -- Put new windows below current
vim.o.splitright = true               -- Put new windows right of current
--vim.o.wildmode = {'list', 'longest'}  -- Command-line completion mode
vim.o.wrap = false                    -- Disable line wrap
vim.o.autochdir = true

vim.o.updatetime = 300

-- vim.o.winbar = "%{%v:lua.require'user.winbar'.get_winbar()%}"
-- vim.o.fillchars="vert:\\ "

-- set colorcolumn=80   -- Highlight 80th column
-- vim.g.indent_blankline_char = '┊'

vim.g.indent_blankline_char = '│'

-- Highlight on yank
vim.api.nvim_exec(
  [[
  augroup YankHighlight
    autocmd!
    autocmd TextYankPost * silent! lua vim.highlight.on_yank()
  augroup end
]],
  false
)

-- When editing a file, always jump to the last cursor position
vim.api.nvim_exec(
  [[
  augroup LastPos
    autocmd!
    autocmd BufReadPost * silent! if line("'\"") | exe "'\"" | endif
  augroup end
]],
  false
)

    -- au! BufRead,BufNewFile *.v,*.vh,*.sv,*.svh,*.svp,*.svi setfiletype systemverilog
vim.api.nvim_exec(
  [[
  augroup VerilogFileType
    autocmd!
    autocmd BufNewFile,BufRead *.v,*.vh,*.sv,*.svh,*.svp,*.svi set filetype=systemverilog
  augroup end
  ]],
  false
)

vim.api.nvim_create_augroup("_cursorline", {})
vim.api.nvim_create_autocmd(
  { "VimEnter", "WinEnter", "BufWinEnter"},
  {
    group = "_cursorline",
    callback = function()
      vim.api.nvim_set_option_value("cursorline", true, { scope = "local" })
    end,
  }
)
vim.api.nvim_create_autocmd(
  { "WinLeave"},
  {
    group = "_cursorline",
    callback = function()
      vim.api.nvim_set_option_value("cursorline", false, { scope = "local" })
    end,
  }
)

function reset_colorizer()
  package.loaded['colorizer'] = nil; require('colorizer').setup(); require('colorizer').attach_to_buffer(0)
end

vim.cmd [[command! ResetColorizer lua reset_colorizer() ]]

-- vim.notify = require("notify")

-- get the syntax group name under cursor
function SynStack()
  vim.cmd [[
    if exists("*synstack")
      echo map(synstack(line('.'), col('.')), 'synIDattr(v:val, "name")')
    endif
  ]]
end

vim.cmd [[ command! GetSyn lua SynStack() ]]

