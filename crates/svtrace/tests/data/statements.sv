// One site per signal per statement: a statement is an assignment, a continuous assign, a port
// connection, or the condition of an if or case. A signal read in several places of one is one site, the first.
module leaf(input logic [1:0] i, output logic o);
    assign o = i[0] ^ i[1];
endmodule

module top(input logic clk, input logic a, input logic b, input logic sel, input logic k,
           output logic y, output logic z, output logic w, output logic q, output logic r, output logic s,
           output logic t);
    // An assignment over three lines, reading a twice.
    assign y = a &
               b |
               a;

    // A condition over three lines, reading sel twice (a control read), and an assignment under it.
    always_comb begin
        z = 1'b0;
        if (sel &&
            k ||
            sel)
            z = b;
    end

    // Two statements read a: a site in each.
    assign w = a;
    assign q = ~a;

    // A case: the selector and an item's expression are statements of their own.
    always_comb begin
        r = 1'b0;
        case (sel)
            a,
            a: r = b;
            default: r = 1'b1;
        endcase
    end

    // A port connection over two lines, reading a twice.
    logic leaf_out;
    leaf u(.i({a,
               a}), .o(leaf_out));
    assign s = leaf_out;

    // A clocked assignment over two lines, reading a twice.
    always_ff @(posedge clk)
        t <= a &
             a;
endmodule
