return {"folke/snacks.nvim",
  ---@type snacks.Config
  priority = 1000,
  lazy = false,
  opts = {
    -- Eshvi's native picker replaced snacks' (lua/picker_keys.lua has its keys).
    -- Still snacks': indent guides, images and lazygit.
    picker = { enabled = false },
    indent = { enabled = true },
    image  = { enabled = true },
    styles = {
      -- lazygit float uses the editor background
      lazygit = { border = "rounded", wo = { winhighlight = "NormalFloat:Normal" } },
    },

    -- scope = { enabled = true },
  },
  keys = {
    { "<leader>gg", function() Snacks.lazygit() end, desc = "Lazygit" },
  },
}
