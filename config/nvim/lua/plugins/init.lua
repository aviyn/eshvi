return {
  { 'echasnovski/mini.icons',
    lazy = true,
    opts = {
      file = {
        [".keep"] = { glyph = "󰊢", hl = "MiniIconsGrey" },
        ["devcontainer.json"] = { glyph = "", hl = "MiniIconsAzure" },
      },
      filetype = {
        dotenv = { glyph = "", hl = "MiniIconsYellow" },
      },
    },
    init = function()
      package.preload["nvim-web-devicons"] = function()
        require("mini.icons").mock_nvim_web_devicons()
        return package.loaded["nvim-web-devicons"]
      end
    end,
  },
  { 'echasnovski/mini.pairs', version = '*',
    event = "InsertEnter",
    opts = {mappings = {["'"] = false}},
  },
  { 'echasnovski/mini.cursorword', version = '*',
    opts = {}
  },
  { 'echasnovski/mini.surround', version = '*',
    keys = function(_, keys)
      -- Populate the keys based on the user's options
      local opts = {
        mappings = {
          add = "gsa", -- Add surrounding in Normal and Visual modes
          delete = "gsd", -- Delete surrounding
          find = "gsf", -- Find surrounding (to the right)
          find_left = "gsF", -- Find surrounding (to the left)
          highlight = "gsh", -- Highlight surrounding
          replace = "gsr", -- Replace surrounding
          update_n_lines = "gsn", -- Update `n_lines`
        },
      }
      local mappings = {
        { opts.mappings.add, desc = "Add Surrounding", mode = { "n", "v" } },
        { opts.mappings.delete, desc = "Delete Surrounding" },
        { opts.mappings.find, desc = "Find Right Surrounding" },
        { opts.mappings.find_left, desc = "Find Left Surrounding" },
        { opts.mappings.highlight, desc = "Highlight Surrounding" },
        { opts.mappings.replace, desc = "Replace Surrounding" },
        { opts.mappings.update_n_lines, desc = "Update `MiniSurround.config.n_lines`" },
      }
      mappings = vim.tbl_filter(function(m)
        return m[1] and #m[1] > 0
      end, mappings)
      return vim.list_extend(mappings, keys)
    end,
    opts = {
      mappings = {
        add = "gsa", -- Add surrounding in Normal and Visual modes
        delete = "gsd", -- Delete surrounding
        find = "gsf", -- Find surrounding (to the right)
        find_left = "gsF", -- Find surrounding (to the left)
        highlight = "gsh", -- Highlight surrounding
        replace = "gsr", -- Replace surrounding
        update_n_lines = "gsn", -- Update `n_lines`
      },
    }
  },
  {"folke/flash.nvim",
    event = "VeryLazy",
    ---@type Flash.Config
    opts = {},
    config = function()
      require('flash').setup({
        modes = {
          search = { enabled = false},
          char = { enabled = false}
        }
      })
    end,
    keys = {
      {
        "s",
        mode = { "n", "x", "o" },
        function()
          require("flash").jump({ search = { forward = true, wrap = false, multi_window = false }, })
        end,
      },
      {
        "S",
        mode = { "n", "x", "o" },
        function ()
          require("flash").jump({ search = { forward = false, wrap = false, multi_window = false }, })
        end
      },
    },
  },
  -- The `main` branch (Neovim 0.12+): it only installs parsers (needs the
  -- `tree-sitter` CLI); highlighting is started per buffer below. It does
  -- not support lazy-loading.
  {'nvim-treesitter/nvim-treesitter', branch = 'main', lazy = false, build = ':TSUpdate',
    config = function()
      require('nvim-treesitter').install({
        'c', 'cpp', 'python', 'lua', 'bash', 'make', 'markdown', 'markdown_inline',
        'json', 'vim', 'vimdoc', 'yaml', 'systemverilog', 'rust', 'toml',
      })
      vim.api.nvim_create_autocmd('FileType', {
        callback = function(ev)
          -- No-op for filetypes without an installed parser.
          pcall(vim.treesitter.start, ev.buf)
        end,
      })
    end
  },
 -- Git integration
  {'lewis6991/gitsigns.nvim',
    event = 'BufRead',
    dependencies = {'nvim-lua/plenary.nvim'},
    config = function() require'gitsigns'.setup() end
  },
  {'nvim-tree/nvim-tree.lua',
    lazy = false,
    cmd = { "NvimTreeToggle", "NvimTreeFindFile" },
    -- dependencies = { "nvim-tree/nvim-web-devicons" },
    config = function()
      -- require("nvim-web-devicons").setup()
      require("nvim-tree").setup {
        disable_netrw = true,
        hijack_netrw = true,
        hijack_cursor = true,
        view = {
          width = 30
        }
      }
    end,
    keys = {
      {
        "<leader>e", "<cmd>NvimTreeToggle<cr>", desc = "Explorer",
      },
    }
  },
  {'numToStr/Navigator.nvim',
    config = function()
      require('Navigator').setup({mux = "auto"})
      -- In Eshvi, ⌃hjkl move between windows and, at their edge, into the GUI's side bar and
      -- bottom panel (`Eshvi.navigate`); anywhere else Navigator moves between windows and
      -- tmux panes.
      local function go(dir, navigator)
        return function()
          if _G.Eshvi and Eshvi.navigate then
            Eshvi.navigate(dir)
          else
            vim.cmd(navigator)
          end
        end
      end
      vim.keymap.set({'n', 't'}, '<C-h>', go('h', 'NavigatorLeft'))
      vim.keymap.set({'n', 't'}, '<C-l>', go('l', 'NavigatorRight'))
      vim.keymap.set({'n', 't'}, '<C-k>', go('k', 'NavigatorUp'))
      vim.keymap.set({'n', 't'}, '<C-j>', go('j', 'NavigatorDown'))
      vim.keymap.set({'n', 't'}, '<C-\\>', '<CMD>NavigatorPrevious<CR>')
    end
  },
  {"folke/ts-comments.nvim",
    event = "VeryLazy",
    opts = {},
  },
  {"folke/todo-comments.nvim",
    dependencies = { "nvim-lua/plenary.nvim" },
    opts = {
      -- your configuration comes here
      -- or leave it empty to use the default settings
      -- refer to the configuration section below
    },
  },

}

