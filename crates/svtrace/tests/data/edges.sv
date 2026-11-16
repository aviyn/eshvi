// The edges of the rule set: inout and ref ports, a task's output argument, a sensitivity list
// without edges, a type parameter's port, an assignment inside an expression, a specparam, escaped
// names, and paths into an unpacked array and struct.
typedef struct packed { logic [3:0] hi; logic [3:0] lo; } pair_t;
typedef struct { logic [3:0] f; logic g; } rec_t;

module pad_cell(inout wire io, input logic oe, input logic o, output logic i);
    assign io = oe ? o : 1'bz;
    assign i = io;
endmodule

module refmod(ref logic r, input logic d, output logic q);
    always_comb q = r;
    always_ff @(posedge d) r <= 1'b1;
endmodule

module sub #(parameter type T = logic [7:0]) (input T d, output T q);
    assign q = d;
endmodule

module leaf(input logic x, output logic y);
    assign y = x;
endmodule

module top(
    inout wire pad,
    inout wire [1:0] bus,
    input logic clk, a, b, k1, s, t_in, lw_a, mx_r,
    input logic [1:0] av,
    input logic [3:0] oe, o,
    input pair_t p,
    output pair_t pq,
    output logic [3:0] i,
    output logic tk, tn, lv, lw, mx, ny, nz, rq, sp_y, \esc-out , eo
);
    // One net on the inout ports of two instances, and a bus split over two more.
    pad_cell u0(.io(pad), .oe(oe[0]), .o(o[0]), .i(i[0]));
    pad_cell u1(.io(pad), .oe(oe[1]), .o(o[1]), .i(i[1]));
    pad_cell u2(.io(bus[0]), .oe(oe[2]), .o(o[2]), .i(i[2]));
    pad_cell u3(.io(bus[1]), .oe(oe[3]), .o(o[3]), .i(i[3]));

    // A ref port is the connected variable itself, read and written inside.
    logic rv;
    refmod ur(.r(rv), .d(a), .q(rq));

    // A port whose type is a type parameter.
    sub #(.T(pair_t)) ut(.d(p), .q(pq));

    // A task writes its output argument from its inputs; with none, from no signal.
    task automatic setit(output logic o, input logic i);
        o = i;
    endtask
    task automatic clear(output logic o);
        o = 1'b0;
    endtask
    always_comb setit(tk, av[1]);
    always_comb clear(tn);

    // A sensitivity list without edges is no clock: the block is combinational.
    always @(a or b or k1) lv = a & b | k1;
    always @(lw_a) lw = lw_a;
    // With an edge in it, the whole list is clocks.
    always @(posedge clk or mx_r) mx <= a;

    // An assignment inside an expression.
    logic nb;
    always_comb begin
        ny = (nb = a) + b;
    end
    assign nz = nb;

    // A specparam is a constant.
    specparam sp = 1'b1;
    assign sp_y = t_in & sp;

    // Escaped names, in a signal and in an instance.
    logic \esc-sig ;
    assign \esc-sig = s;
    assign \esc-out = \esc-sig ;
    leaf \esc-inst (.x(s), .y(eo));

    // Unpacked: an array of bytes and a struct.
    logic [7:0] mem [4];
    rec_t rec;
    always_ff @(posedge clk) mem[av] <= {8{a}};
    always_ff @(posedge clk) rec.f <= {4{b}};
endmodule
