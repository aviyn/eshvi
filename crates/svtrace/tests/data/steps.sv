// svtrace.step and svtrace.endpoints: chains, a loop, constants, "many", and the depth limit.
package kinds;
    typedef enum logic [1:0] { IDLE, BUSY } state_e;
endpackage

module source(input logic i, output logic o);
    assign o = i;
endmodule

module steps_top(input logic seed, input logic [3:0] bus, output logic result);
    import kinds::*;

    // A chain of plain copies, 27 signals long: followed to the depth limit.
    logic c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12, c13, c14, c15, c16, c17, c18, c19, c20, c21, c22, c23, c24, c25, c26;
    assign c0 = seed;
    assign c1 = c0;
    assign c2 = c1;
    assign c3 = c2;
    assign c4 = c3;
    assign c5 = c4;
    assign c6 = c5;
    assign c7 = c6;
    assign c8 = c7;
    assign c9 = c8;
    assign c10 = c9;
    assign c11 = c10;
    assign c12 = c11;
    assign c13 = c12;
    assign c14 = c13;
    assign c15 = c14;
    assign c16 = c15;
    assign c17 = c16;
    assign c18 = c17;
    assign c19 = c18;
    assign c20 = c19;
    assign c21 = c20;
    assign c22 = c21;
    assign c23 = c22;
    assign c24 = c23;
    assign c25 = c24;
    assign c26 = c25;

    // A loop of two signals.
    logic la, lb;
    assign la = lb;
    assign lb = la;

    // A flop: its data is the one step that counts, its clock does not.
    logic clk, flop_d, flop_q;
    always_ff @(posedge clk) flop_q <= flop_d;
    assign flop_d = seed;

    // Constants: a package enum member, a module-local one, a parameter, a literal.
    typedef enum logic { L_A, L_B } local_e;
    localparam logic PARAM = 1'b1;
    kinds::state_e st;
    local_e loc;
    logic with_param, with_literal;
    assign st = BUSY;
    assign loc = L_B;
    assign with_param = PARAM & seed;
    assign with_literal = 1'b0;

    // Nine signals in nine instances: "many", so not followed.
    source u0(.i(seed), .o());
    source u1(.i(seed), .o());
    source u2(.i(seed), .o());
    source u3(.i(seed), .o());
    source u4(.i(seed), .o());
    source u5(.i(seed), .o());
    source u6(.i(seed), .o());
    source u7(.i(seed), .o());
    source u8(.i(seed), .o());
    logic many, eight;
    assign eight = u0.o ^ u1.o ^ u2.o ^ u3.o ^ u4.o ^ u5.o ^ u6.o ^ u7.o;
    assign many = u0.o ^ u1.o ^ u2.o ^ u3.o ^ u4.o ^ u5.o ^ u6.o ^ u7.o ^ u8.o;

    // Nine signals in one instance: not "many", so each is followed.
    logic f0, f1, f2, f3, f4, f5, f6, f7, f8, flat;
    assign flat = f0 ^ f1 ^ f2 ^ f3 ^ f4 ^ f5 ^ f6 ^ f7 ^ f8;
    assign {f0, f1, f2, f3, f4, f5, f6, f7, f8} = {9{seed}};

    // Where the chain starts one step in.
    logic head, tail;
    assign head = bus[0];
    assign tail = head;
    assign result = tail & many & flat & flop_q & with_param & with_literal & st[0] & loc & lb;
endmodule
