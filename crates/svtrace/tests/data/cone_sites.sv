// From slang-server's tests/cpp/ConeTests.cpp (the cone_sites.sv source written inline there), cone-connection-sites
// branch of the aviyn/slang-server fork. SPDX-FileCopyrightText: Hudson River Trading. SPDX-License-Identifier: MIT
`define ASSIGN(dst, src) assign dst = src
`define ASSIGN_FIXED(dst) assign dst = fixed_src

module child(input logic in, output logic out);
    assign out = in;
endmodule

module wide_child(input logic in, output logic [1:0] pair);
    assign pair = {in, in};
endmodule

module wild_child(input logic wild_in, output logic wild_out);
    assign wild_out = wild_in;
endmodule

module top;
    logic data_src, data_dst, twice_src, twice_dst, en, gated, gated_d;
    logic sel, cased, named_src, named_dst, in, out, high, low;
    logic wild_in, wild_out, macro_src, macro_dst, fixed_src, fixed_dst;
    logic wire_src, wire_dst, comb_src, comb_dst, flop_d, flop_q, cond_src, cond_dst, clk;
    logic [1:0] sliced, shrunk;
    logic [3:0] whole_a, whole_b;
    logic last;

    assign wire_dst = wire_src;
    always_comb comb_dst = comb_src;
    always_ff @(posedge clk) flop_q <= flop_d;
    always_comb if (en) cond_dst = cond_src;
    assign sliced = whole_a[1:0];
    assign shrunk = whole_b;

    assign data_dst = data_src + 1'b1;

    always_comb begin
        twice_dst = twice_src;
        twice_dst = ~twice_src;
    end

    always_comb begin
        gated = 1'b0;
        if (en) gated = gated_d;
    end

    always_comb begin
        case (sel)
            1'b0: cased = 1'b1;
            default: cased = 1'b0;
        endcase
    end

    child named(.in (named_src), .out (named_dst));
    child implicit(.in, .out);
    wide_child concat(.in (1'b0), .pair ({high, low}));
    wild_child wild(.*);

    `ASSIGN(macro_dst, macro_src);
    `ASSIGN_FIXED(fixed_dst);

    parameter logic PARAM = 1'b1;
    typedef enum logic { IDLE, BUSY } state_e;
    state_e state;
    logic lit, param_dst, reset_flop, counted, rd_only;

    assign lit = 1'b1;
    assign param_dst = PARAM;
    assign state = IDLE;
    always_ff @(posedge clk) begin
        if (en) reset_flop <= '0;
        else reset_flop <= flop_d;
    end
    always_comb begin
        counted = 1'b0;
        counted += 1'b1;
    end

    function logic make_bit();
        return 1'b1;
    endfunction
    logic from_call;
    assign from_call = make_bit();

    logic dflt_src, dflt_alt, dflt_dst, star_src, star_dst, idx, idx_src, mem_d;
    logic [3:0] mem, mem2, bus;
    always_comb begin
        dflt_dst = dflt_src;
        if (en) dflt_dst = dflt_alt;
    end
    always @* star_dst = star_src;
    always_comb begin
        idx = idx_src;
        mem[idx] = 1'b0;
        mem2[idx] = mem_d;
    end

    child logic_port(.in (en & sel), .out ());
    child bit_port(.in (bus[0]), .out ());
    child picked_port(.in (bus[sel]), .out ());
    wide_child padded_port(.in ({bus[1:1]}), .pair ());
endmodule
