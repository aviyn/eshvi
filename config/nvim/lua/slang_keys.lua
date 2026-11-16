-- The keys of Eshvi's SystemVerilog support (`:EshviSlang`, defined by the app when it runs Neovim with
-- `[neovim] config = "own"`; the Hierarchy and Trace views come with a slang-server or svtrace client).
-- They used to be slang-server.nvim's and this config's `plugins/slang_server.lua`.
local keys = {
  { "<leader>vh", "hierarchy", "Slang: design hierarchy" },
  { "<leader>vs", "search", "Slang: search the hierarchy" },
  { "<leader>vu", "up", "Slang: go up to the instantiation" },
  { "<leader>vt", "drivers", "Slang: trace the drivers of the signal under the cursor" },
  { "<leader>vT", "loads", "Slang: trace the loads of the signal under the cursor" },
  { "<leader>ve", "driverEndpoints", "Slang: where the signal under the cursor is assigned, past the wiring" },
  { "<leader>vE", "loadEndpoints", "Slang: where the signal under the cursor is read, past the wiring" },
}

for _, key in ipairs(keys) do
  vim.keymap.set("n", key[1], "<cmd>EshviSlang " .. key[2] .. "<cr>", { desc = key[3], silent = true })
end

return keys
