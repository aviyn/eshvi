return { 
  'EdenEast/nightfox.nvim',
  lazy = false, -- make sure we load this during startup if it is your main colorscheme
  priority = 1000, -- make sure to load this before all the other start plugins
  config = function ()
    local palettes = {
      -- Everything defined under `all` will be applied to each style.
      all = {
        -- Each palette defines these colors:
        --   black, red, green, yellow, blue, magenta, cyan, white, orange, pink
        --
        -- These colors have 3 shades: base, bright, and dim
        --
        -- Defining just a color defines it's base color
        -- red = "#ff0000",
      },
      nightfox = {
        -- A specific style's value will be used over the `all`'s value
        red = "#c94f6d",
      },
      dayfox = {
        -- Defining multiple shades is done by passing a table
        blue = { base = "#4d688e", bright = "#4e75aa", dim = "#485e7d" },
      },
      nordfox = {
        -- A palette also defines the following:
        --   bg0, bg1, bg2, bg3, bg4, fg0, fg1, fg2, fg3, sel0, sel1, comment
        --
        -- These are the different foreground and background shades used by the theme.
        -- The base bg and fg is 1, 0 is normally the dark alternative. The others are
        -- incrementally lighter versions.
        bg1 = "#2b303b",

        -- sel is different types of selection colors.
        sel0 = "#3e4a5b", -- Popup bg, visual selection bg
        sel1 = "#4f6074", -- Popup sel bg, search bg

        -- comment is the definition of the comment color.
        comment = "#60728a",
      },
    }
    local specs = {
      nordfox = {
        syntax = {
          -- Value does not start with `#` so is treated as a template.
          -- Since `magenta` is a `shade` object the `base` value will be used.
          -- keyword = "#a292a3",
          -- keyword = "#b6927b",
          operator = "#b6927b",
          number = "#a6a69c",
          -- number = "#949fb5",
          type = "#c4b28a",
          -- Adding either `.bright` or `.dim` will change the value
          -- conditional = "magenta.bright",
          -- number = "orange.dim",
        },
      },
    }
    local options = {
      styles = {
        comments = "italic",
      }
    }
    require('nightfox').setup({ options = options })
    require("nightfox").setup({ palettes = palettes, specs = specs, groups = {} })
    -- vim.cmd('colorscheme nightfox')
  end,
}
