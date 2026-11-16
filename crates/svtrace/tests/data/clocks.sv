// Clocks (svtrace.step, svtrace.endpoints and the `clock` site kind): a clocked block with a reset next
// to an unclocked block in one module, a flop chain through instances, and a clock passed down.
module mixed(
    input logic clk,
    input logic rst_n,
    input logic en,
    input logic [3:0] a,
    output logic [3:0] reg_q,
    output logic [3:0] comb_o
);
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) reg_q <= '0;
        else if (en) reg_q <= a;
    end

    always_comb begin
        comb_o = a + 4'd1;
    end
endmodule

module stage(input logic clk, input logic d, output logic q);
    always_ff @(posedge clk) q <= d;
endmodule

module clk_top(input logic clk, input logic rst_n, input logic en, input logic [3:0] a, output logic [3:0] reg_q, output logic [3:0] comb_o, input logic din, output logic dout);
    mixed u_mixed(.clk, .rst_n, .en, .a, .reg_q, .comb_o);

    logic mid;
    stage u_first(.clk, .d(din), .q(mid));
    stage u_second(.clk, .d(mid), .q(dout));
endmodule
