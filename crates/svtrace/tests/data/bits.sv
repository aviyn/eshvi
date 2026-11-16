// Bits and fields: selects, part selects both ways, packed structs with nested fields,
// concatenations on both sides, a port connected to a part, a non-constant select.
package bits_pkg;
  typedef struct packed {
    logic [3:0] hi;
    logic [3:0] lo;
  } byte_t;

  typedef struct packed {
    logic       en;
    byte_t      b;
    logic [1:0] mode;
  } cfg_t;
endpackage

module bits_leaf (
    input  logic [7:0] a,
    output logic [7:0] y
);
  assign y = a;
endmodule

module bits_top (
    input logic       clk,
    input logic [7:0] a_in,
    input logic [3:0] b_in,
    input logic [7:0] d_in,
    input logic [3:0] p_in,
    input logic [3:0] q_in,
    input logic       en_in,
    input logic [3:0] hi_in,
    input logic [3:0] lo_in,
    input logic [1:0] mode_in,
    input logic [2:0] idx
);
  import bits_pkg::*;

  // Part selects and single bits, descending and ascending.
  logic [7:0] v;
  logic [0:7] up;
  assign v[3:0] = a_in[7:4];
  assign v[7:4] = b_in;
  assign up[0:3] = a_in[3:0];
  assign up[4] = a_in[7];
  assign up[5:7] = b_in[2:0];

  // Loads of the parts.
  logic [3:0] v_low, v_high;
  logic       v_bit;
  assign v_low  = v[3:0];
  assign v_high = v[7:4];
  assign v_bit  = v[5];

  // A packed struct with nested fields.
  cfg_t cfg;
  assign cfg.en      = en_in;
  assign cfg.b.hi    = hi_in;
  assign cfg.b.lo    = lo_in;
  assign cfg.mode    = mode_in;
  logic [3:0] hi_out;
  logic [1:0] mode_out;
  assign hi_out   = cfg.b.hi;
  assign mode_out = cfg.mode;

  // Concatenations on the left and on the right.
  logic [3:0] c1, c2;
  assign {c1, c2} = d_in;
  logic [7:0] pq;
  assign pq = {p_in, q_in};
  logic [7:0] rep;
  assign rep = {2{p_in}};
  logic [3:0] split_hi, split_lo;
  assign split_hi = pq[7:4];
  assign split_lo = pq[3:0];

  // A port connected to a part, and its output to another part.
  logic [7:0] leaf_out;
  logic [7:0] leaf_in;
  assign leaf_in[3:0] = p_in;
  assign leaf_in[7:4] = q_in;
  logic [7:0] part_out;
  bits_leaf u_leaf (
      .a(leaf_in),
      .y(part_out)
  );
  logic [3:0] leaf_low;
  logic [7:0] wide;
  bits_leaf u_part (
      .a({4'h0, p_in}),
      .y(wide)
  );
  assign leaf_low = wide[3:0];

  // A non-constant select, on both sides.
  logic bit_sel;
  assign bit_sel = d_in[idx];
  logic [7:0] m;
  always_comb begin
    m = 8'h00;
    m[idx] = en_in;
  end

  // A struct built field by field, and a flop that assigns fields from different places.
  cfg_t cfg_n, cfg_q;
  always_comb begin
    cfg_n = '{en: en_in, b: '{hi: hi_in, lo: lo_in}, mode: mode_in};
  end
  always_ff @(posedge clk) begin
    cfg_q <= '{en: 1'b0, b: cfg_n.b, mode: cfg_n.mode};
  end

  // Not packed: a part of it stays the whole signal.
  logic [3:0] mem[4];
  assign mem[1] = p_in;
  logic [3:0] mem_out;
  assign mem_out = mem[2];

  // Text before a use that takes more than one byte or one UTF-16 unit per character.
  logic v_bit2;
  /* é😀 */ assign v_bit2 = v[6];

  // A flop of one field, a condition on one bit.
  logic [7:0] f;
  always_ff @(posedge clk) begin
    if (a_in[0]) f[3:0] <= d_in[3:0];
    f[7:4] <= d_in[7:4];
  end
endmodule
