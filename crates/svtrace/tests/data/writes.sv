// What counts as a write, a read and a condition: reads outside an assignment, loops, declaration
// initializers, ++ and compound assignments, left-side indices, case items that read signals.
module top(
    input logic clk,
    input logic rd_a, rd_b, ln, lp_b, lp_a, wl_c, wl_b, rp_n, rp_b, aa_c, aa_b, aa_d,
    input logic in_a, in_b,
    input logic [3:0] inc_in,
    input logic [1:0] waddr, raddr, i1, i2, pair, sel,
    input logic [7:0] wdata,
    input logic k1, k2, ca, cb,
    output logic rd_y, lp_z, lp_t, wl_y, rp_y, aa_y, loc_y, cy, cz,
    output logic [7:0] rd_mem
);
    // A read that is not in an assignment: an assertion, a system task, a call statement.
    always_comb begin
        assert (rd_a == 1'b0);
        $display("%d", rd_a);
        rd_y = rd_b;
    end

    // A loop's condition controls what the loop assigns, and the statements after it are not its.
    always_comb begin
        lp_z = 1'b0;
        for (int i = 0; i < ln; i++) lp_z = lp_z ^ lp_b;
        lp_t = lp_a;
    end
    always_comb begin
        wl_y = 1'b0;
        while (wl_c) begin
            wl_y = wl_b;
        end
    end
    always_comb begin
        rp_y = 1'b0;
        repeat (rp_n) rp_y = rp_b;
    end

    // An assertion's action blocks run depending on its condition.
    always_comb begin
        assert (aa_c) aa_y = aa_b; else aa_y = aa_d;
    end

    // Declaration assignments: a net's is a continuous assignment, a variable's its first value.
    wire w_net = in_a;
    wire w_expr = in_a & in_b;
    logic v_init = in_b;
    logic v_const = 1'b1;
    always_comb begin
        automatic logic loc = in_a;
        loc_y = loc;
    end

    // ++, -- and += write their operand and read it.
    logic [3:0] cnt, dn, acc;
    always_ff @(posedge clk) cnt++;
    always_ff @(posedge clk) --dn;
    always_ff @(posedge clk) acc += inc_in;

    // A left-side index picks what is written.
    logic [7:0] mem [4];
    always_ff @(posedge clk) mem[waddr] <= wdata;
    assign rd_mem = mem[raddr];
    logic [3:0] sa, sb;
    always_ff @(posedge clk) {sa[i1], sb[i2]} <= pair;

    // Case items that read signals: an item is reached when the ones before it didn't match.
    always_comb begin
        cy = 1'b0;
        cz = 1'b0;
        case (sel)
            {1'b0, k1}: cy = ca;
            {1'b1, k2}: cz = cb;
            default: ;
        endcase
    end
endmodule
