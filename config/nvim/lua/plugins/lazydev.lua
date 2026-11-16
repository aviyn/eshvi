-- Neovim's Lua API (`vim.*`) for lua_ls when editing config/plugin files.
return {
  "folke/lazydev.nvim",
  ft = "lua",
  opts = {
    library = {
      -- `vim.uv` types, loaded only when a file uses it.
      { path = "${3rd}/luv/library", words = { "vim%.uv" } },
    },
  },
}
