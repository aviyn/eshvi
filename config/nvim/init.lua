-- disable netrw at the very start of your init.lua
vim.g.loaded_netrw = 1
vim.g.loaded_netrwPlugin = 1

vim.o.shell = "/opt/homebrew/bin/bash"

local lazypath = vim.fn.stdpath("data") .. "/lazy/lazy.nvim"
if not vim.loop.fs_stat(lazypath) then
  vim.fn.system({
    "git",
    "clone",
    "--filter=blob:none",
    "https://github.com/folke/lazy.nvim.git",
    "--branch=stable", -- latest stable release
    lazypath,
  })
end
vim.opt.rtp:prepend(lazypath)

vim.g.mapleader = " " -- Make sure to set `mapleader` before lazy so your mappings are correct
vim.g.maplocalleader = " " -- Make sure to set `mapleader` before lazy so your mappings are correct

require("lazy").setup("plugins")

require("options")
require("keymap")
require("picker_keys")
require("slang_keys")

vim.cmd('colorscheme nordfox')

vim.api.nvim_set_hl(0, 'MiniCursorword', {underline = true})
vim.api.nvim_set_hl(0, 'MiniCursorwordCurrent', {underline = true})
