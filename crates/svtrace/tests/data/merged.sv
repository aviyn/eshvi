// Merged instances: svtrace.step and svtrace.endpoints with `paths`, one per instance of a module.

// An internal chain, a clock and a port: three instances of it below.
module chain(input logic clk, input logic a, output logic q);
    logic m1, m2;
    assign m1 = a;
    assign m2 = m1;
    always_ff @(posedge clk) q <= m2;
endmodule

// Elaborated differently by a parameter: a branch of its own in one instance only.
module gated #(parameter bit WIDE = 0)(input logic d, output logic o);
    if (WIDE) begin : g_wide
        logic w;
        assign w = ~d;
        assign o = w;
    end else begin : g_narrow
        assign o = d;
    end
endmodule

// Instantiated twice below, its child connected to the same names in each.
module pair(input logic x, input logic clk, output logic z);
    logic x_int;
    assign x_int = x;
    chain c(.clk(clk), .a(x_int), .q(z));
endmodule

module merged_top(input logic clk, input logic x, input logic y, input logic [2:0] vec, output logic [7:0] outs);
    // u1 and u2 are connected alike, u3 differently.
    logic q1, q2, q3;
    chain u1(.clk(clk), .a(x), .q(q1));
    chain u2(.clk(clk), .a(x), .q(q2));
    chain u3(.clk(clk), .a(y), .q(q3));

    // A loop-generated array, each one on its own bit.
    logic [2:0] qs;
    for (genvar i = 0; i < 3; i++) begin : g
        chain c(.clk(clk), .a(vec[i]), .q(qs[i]));
    end

    // A parameter-gated branch.
    logic ow, on;
    gated #(.WIDE(1)) gw(.d(x), .o(ow));
    gated #(.WIDE(0)) gn(.d(x), .o(on));

    // A parent instantiated twice.
    logic z1, z2;
    pair p1(.x(x), .clk(clk), .z(z1));
    pair p2(.x(x), .clk(clk), .z(z2));

    assign outs = {q1, q2, q3, qs, ow, on};
    logic zs;
    assign zs = z1 ^ z2;
endmodule
