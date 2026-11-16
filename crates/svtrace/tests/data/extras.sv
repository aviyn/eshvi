// Constructs the fork's tests don't touch: struct fields, widths of named types, casts,
// hierarchical references, net initializers, loops, tasks and functions.
package defs;
    typedef struct packed {
        logic [3:0] hi;
        logic [3:0] lo;
    } byte_t;
    typedef enum logic [2:0] { A, B, C } mode_e;
endpackage

module leaf(input logic [7:0] in, output logic [7:0] out, inout wire io);
    assign out = in;
endmodule

module top(input logic clk, input logic [7:0] raw, output logic [7:0] result);
    import defs::*;

    byte_t packed_a, packed_b;
    logic [7:0] plain;
    mode_e mode, mode_q;
    logic [2:0] mode_bits;
    logic [7:0] widened, narrowed_src, cast_dst, cast_src;
    logic [3:0] narrowed;
    logic signed [7:0] signed_dst;

    // Same width, different named types: a copy.
    assign plain = packed_a;
    assign packed_b = plain;
    // An enum copied into its own width, and into a plain vector.
    assign mode_bits = mode;
    assign mode_q = mode;
    // Narrower and wider.
    assign narrowed = narrowed_src[3:0];
    assign widened = narrowed;
    // A cast and a signedness change.
    assign cast_dst = 8'(cast_src);
    assign signed_dst = cast_src;

    // Struct fields assigned on their own, then the whole struct in another block.
    always_comb begin
        packed_a.hi = raw[7:4];
        packed_a.lo = raw[3:0];
    end

    // A net with an initializer, and a replication.
    wire [7:0] net_init = raw;
    logic [7:0] repl;
    assign repl = {2{raw[3:0]}};

    // A ternary and a reduction.
    logic tern_sel, tern_a, tern_b, tern_out, red;
    assign tern_out = tern_sel ? tern_a : tern_b;
    assign red = &raw;

    // Hierarchical references down into an instance.
    leaf u_leaf(.in(raw), .out(), .io());
    logic seen;
    assign seen = u_leaf.out[0];

    // A loop with a local index, and a function with a signal argument.
    logic [7:0] looped, loop_src;
    always_comb begin
        for (int i = 0; i < 8; i++) looped[i] = loop_src[7 - i];
    end
    function automatic logic [7:0] twice(input logic [7:0] x);
        return x + x;
    endfunction
    logic [7:0] doubled, doubled_src;
    assign doubled = twice(doubled_src);

    // A clocked block with a reset condition on an input port connection and an output.
    logic rst_n_local;
    always_ff @(posedge clk or negedge rst_n_local) begin
        if (!rst_n_local) result <= '0;
        else result <= raw;
    end

    // Case with an enum and a nested if.
    logic [3:0] decoded;
    always_comb begin
        decoded = '0;
        case (mode)
            A: decoded = 4'd1;
            B: if (tern_sel) decoded = 4'd2; else decoded = {3'b0, red};
            default: decoded = 4'd8;
        endcase
    end
endmodule
