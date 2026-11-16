
-- more mappings are defined in `lua/config/which.lua`
local map = vim.keymap.set
default_options = { silent = true }
expr_options = { expr = true, silent = true }

--Remap space as leader key
map({ "n", "v" }, "<Space>", "<Nop>", { noremap = true, silent = true })
vim.g.mapleader = " "
vim.g.maplocalleader = ' '

--Remap for dealing with visual line wraps
map("n", "k", "v:count == 0 ? 'gk' : 'k'", expr_options)
map("n", "j", "v:count == 0 ? 'gj' : 'j'", expr_options)

-- better indenting
map("v", "<", "<gv", default_options)
map("v", ">", ">gv", default_options)

-- paste over currently selected text without yanking it
map("v", "p", '"_dP', default_options)

-- Mimic shell movements
map('i', '<C-E>', '<ESC>A', default_options)
map('i', '<C-A>', '<ESC>I', default_options)
map('i', '<C-S>', '<ESC>:w<CR>', default_options)
map('n', '<C-S>', ':w<CR>', default_options)

-- Tab switch buffer
-- map("n", "<TAB>", ":bnext<CR>", default_options)
-- map("n", "<S-TAB>", ":bprevious<CR>", default_options)

-- Cancel search highlighting with ESC
map("n", "<ESC>", ":nohlsearch<Bar>:echo<CR>", default_options)

-- Resizing panes
map("n", "<M-Left>", ":vertical resize +1<CR>", default_options)
map("n", "<M-Right>", ":vertical resize -1<CR>", default_options)
map("n", "<M-Up>", ":resize -1<CR>", default_options)
map("n", "<M-Down>", ":resize +1<CR>", default_options)

-- map({"x", "n", "i"}, "<Left>", "<Nop>", default_options)
-- map({"x", "n", "i"}, "<Right>", "<Nop>", default_options)
-- map({"x", "n", "i"}, "<Up>", "<Nop>", default_options)
-- map({"x", "n", "i"}, "<Down>", "<Nop>", default_options)

-- Autocorrect spelling from previous error
map("i", "<c-f>", "<c-g>u<Esc>[s1z=`]a<c-g>u", default_options)

-- Move selected line / block of text in visual mode
-- map("x", "K", ":move '<-2<CR>gv-gv", default_options)
-- map("x", "J", ":move '>+1<CR>gv-gv", default_options)

-- Window navigation
map("n", "<M-h>", "<C-W>h", default_options)
map("n", "<M-l>", "<C-W>l", default_options)
map("n", "<M-k>", "<C-W>k", default_options)
map("n", "<M-j>", "<C-W>j", default_options)

--[[ vim.api.nvim_set_keymap('i', '<M-j>', '<Esc>:m .+1<cr>==gi', { silent = true})
vim.api.nvim_set_keymap('i', '<M-k>', '<Esc>:m .-2<cr>==gi', { silent = true})
vim.api.nvim_set_keymap('n', '<M-j>', ':m .+1<cr>==', { silent = true})
vim.api.nvim_set_keymap('n', '<M-k>', ':m .-2<cr>==', { silent = true})
vim.api.nvim_set_keymap('v', '<M-j>', ":m '>+1<cr>gv-gv", { silent = true})
vim.api.nvim_set_keymap('v', '<M-k>', ":m '<-2<cr>gv-gv", { silent = true}) ]]



-- Max height
-- map('n', '<leader>-', '<C-w>_')
-- Max width leader+|
-- map('n', '<leader>\\', '<C-w><bar>')
-- Fair defaults
-- nnoremap <leader>= <c-w>=
-- nnoremap <leader>v :edit $MYVIMRC

-- noremap <C-s> :w<cr>
-- inoremap <C-s> <Esc>:w<cr>a

-- ⌃hjkl in a terminal buffer: to the window beside it; in Eshvi, at the edge of the
-- windows, into the GUI's side bar or bottom panel.
function _G.window_navigate(dir)
  if _G.Eshvi and Eshvi.navigate then
    Eshvi.navigate(dir)
  else
    vim.cmd('wincmd ' .. dir)
  end
end

function _G.set_terminal_keymaps()
  -- lazygit needs <esc>, j/k and <C-*> for itself
  if vim.api.nvim_buf_get_name(0):match('//%d+:%S*lazygit') then return end
  local opts = {noremap = true}
  vim.api.nvim_buf_set_keymap(0, 't', '<esc>', [[<C-\><C-n>]], opts)
  vim.api.nvim_buf_set_keymap(0, 't', 'jk', [[<C-\><C-n>]], opts)
  vim.api.nvim_buf_set_keymap(0, 't', '<C-h>', [[<C-\><C-n><Cmd>lua window_navigate('h')<CR>]], opts)
  vim.api.nvim_buf_set_keymap(0, 't', '<C-j>', [[<C-\><C-n><Cmd>lua window_navigate('j')<CR>]], opts)
  vim.api.nvim_buf_set_keymap(0, 't', '<C-k>', [[<C-\><C-n><Cmd>lua window_navigate('k')<CR>]], opts)
  vim.api.nvim_buf_set_keymap(0, 't', '<C-l>', [[<C-\><C-n><Cmd>lua window_navigate('l')<CR>]], opts)
end

-- if you only want these mappings for toggle term use term://*toggleterm#* instead
vim.cmd('autocmd! TermOpen term://* lua set_terminal_keymaps()')




local opts = {noremap = true}
vim.api.nvim_set_keymap('n', '<Leader>va', [[<cmd>lua require('user/tools').run_verilog_auto(true)<cr>]], opts)
vim.api.nvim_set_keymap('n', '<Leader>vd', [[<cmd>lua require('user/tools').run_verilog_auto(false)<cr>]], opts)

vim.api.nvim_set_keymap('n', '<Leader>vc', [[<cmd>lua package.loaded['user.vutils']=nil <cr>]], opts)
vim.api.nvim_set_keymap('n', '<Leader>vl', [[<cmd>lua require('user/vutils').declare() <cr>]], opts)
vim.api.nvim_set_keymap('n', '<Leader>vi', [[<cmd>lua require('user/vutils').initialize() <cr>]], opts)

vim.keymap.set('n', 'gl', vim.diagnostic.open_float)
vim.keymap.set('n', '[d', vim.diagnostic.goto_prev)
vim.keymap.set('n', ']d', vim.diagnostic.goto_next)
vim.keymap.set('n', '<space>q', vim.diagnostic.setloclist)
