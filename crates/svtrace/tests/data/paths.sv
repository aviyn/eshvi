// Hierarchical paths: packages, generate blocks and loops, instance arrays, escaped names.
package pkg;
    logic pkg_sig;
endpackage

module leaf(input logic d, output logic q);
    logic inner;
    assign inner = d;
    assign q = inner;
endmodule

module top(input logic clk, input logic d, output logic q);
    parameter int P = 1;
    parameter int Q = 2;

    logic pkg_copy;
    assign pkg_copy = pkg::pkg_sig;

    // A named loop that does not start at zero.
    for (genvar i = 2; i < 5; i++) begin : gen_x
        logic v;
        assign v = d;
    end

    // A loop counting through negative values.
    for (genvar n = -1; n < 1; n++) begin : gen_neg
        logic w;
        assign w = d;
    end

    // Two unnamed generate blocks take the names slang gives them.
    if (P == 1) begin
        logic first_if;
        assign first_if = d;
    end
    if (Q == 2) begin
        logic second_if;
        assign second_if = d;
    end

    // Nested loops with an instance in the inner one.
    for (genvar r = 0; r < 2; r++) begin : outer
        for (genvar c = 0; c < 2; c++) begin : inner_loop
            logic z;
            leaf u_in_gen(.d(z), .q());
        end
    end

    // A case generate with a name.
    case (P)
        1: begin : case_one
            logic only_one;
            assign only_one = d;
        end
        default: begin : case_other
            logic other;
        end
    endcase

    // Instance arrays: descending, ascending from non-zero, and two-dimensional.
    logic [3:0] bus_d, bus_q;
    leaf u_desc [3:0] (.d(bus_d), .q(bus_q));
    leaf u_asc [2:4] (.d(bus_d[2:0]), .q());
    leaf u_2d [1:0][2:3] (.d(bus_d[0]), .q());

    // An escaped identifier.
    logic \esc$sig ;
    assign \esc$sig = d;

    assign q = bus_q[0];
endmodule
